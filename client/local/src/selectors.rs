use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::get_config_dir;

// ---------------------------------------------------------------------------
// URL normalisation (same logic as savhub-server)
// ---------------------------------------------------------------------------

/// Convert any repo URL to the canonical registry sign format: `domain/owner/repo`.
///
/// - `https://github.com/org/repo` → `github.com/org/repo`
/// - `https://github.com/org/repo.git` → `github.com/org/repo`
/// - `git@github.com:org/repo` → `github.com/org/repo`
/// - `github.com/org/repo` → `github.com/org/repo`
pub fn normalize_repo_url_to_sign(raw: &str) -> String {
    let normalized = normalize_git_url(raw);
    // Strip https:// prefix and .git suffix
    normalized
        .strip_prefix("https://")
        .or_else(|| normalized.strip_prefix("http://"))
        .unwrap_or(&normalized)
        .trim_end_matches(".git")
        .trim_end_matches('/')
        .to_string()
}

/// Normalize a git URL to a canonical HTTPS form.
///
/// - `git@github.com:org/repo` → `https://github.com/org/repo.git`
/// - `https://github.com/org/repo` → `https://github.com/org/repo.git`
/// - `http://github.com/org/repo.git/` → `https://github.com/org/repo.git`
pub fn normalize_git_url(raw: &str) -> String {
    let url = raw.trim();
    // Strip URL fragment (#...) and query string (?...)
    let url = url.split('#').next().unwrap_or(url);
    let url = url.split('?').next().unwrap_or(url);
    let url = url.trim_end_matches('/');

    // git@host:path → https://host/path
    let url = if let Some(rest) = url.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            format!("https://{}/{}", host, path.trim_start_matches('/'))
        } else {
            url.to_string()
        }
    } else if let Some(rest) = url.strip_prefix("http://") {
        // Upgrade http → https
        format!("https://{rest}")
    } else if !url.starts_with("https://") {
        // Bare host/path — assume https
        format!("https://{url}")
    } else {
        url.to_string()
    };

    // Strip trailing slash again after transform
    let url = url.trim_end_matches('/').to_string();

    // Ensure .git suffix
    if url.ends_with(".git") {
        url
    } else {
        format!("{url}.git")
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single rule condition for a selector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectorRule {
    /// Check that a file exists relative to the folder scope.
    FileExists { path: String },
    /// Check that a sub-folder exists relative to the folder scope.
    FolderExists { path: String },
    /// Check that at least one file matching the glob pattern exists.
    GlobMatch { pattern: String },
    /// Check that a file contains a specific string (case-sensitive substring match).
    FileContains { path: String, contains: String },
    /// Check that a file's content matches a regular expression.
    FileRegex { path: String, pattern: String },
    /// Check that an environment variable is set (non-empty).
    EnvVarSet { name: String },
    /// Run a shell command and check that it exits with code 0.
    CommandExits { command: String },
}

/// Mode for how rules are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    AllMatch,
    AnyMatch,
    Custom,
}

/// A composable boolean expression tree over selector rules.
///
/// Rules are referenced by 0-based index into `SelectorDefinition.rules`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RuleExpression {
    /// Reference to a rule by index.
    Check { index: usize },
    /// All operands must evaluate to true.
    And { operands: Vec<RuleExpression> },
    /// At least one operand must evaluate to true.
    Or { operands: Vec<RuleExpression> },
    /// Negation.
    Not { operand: Box<RuleExpression> },
}

/// A repository reference used in selectors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SelectorRepo {
    pub git_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
}

impl SelectorRepo {
    pub fn from_url(url: &str) -> Self {
        Self {
            git_url: url.to_string(),
            git_sha: None,
            git_branch: None,
        }
    }
}

/// A skill or flock reference used in selectors.
/// Uses `{repo, path}` to uniquely identify the resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SelectorSkillRef {
    pub repo: String,
    pub path: String,
}

impl std::fmt::Display for SelectorSkillRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.repo)
        } else {
            write!(f, "{}/{}", self.repo, self.path)
        }
    }
}

impl SelectorSkillRef {
    /// Parse a `"domain/owner/repo/path"` string into `{repo, path}`.
    /// Splits on the third `/` to separate repo from path.
    pub fn parse(input: &str) -> Self {
        let trimmed = input
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let mut slashes = 0;
        let mut split_at = None;
        for (i, ch) in trimmed.char_indices() {
            if ch == '/' {
                slashes += 1;
                if slashes == 3 {
                    split_at = Some(i);
                    break;
                }
            }
        }
        if let Some(pos) = split_at {
            Self {
                repo: trimmed[..pos].to_string(),
                path: trimmed[pos + 1..].to_string(),
            }
        } else {
            Self {
                repo: trimmed.to_string(),
                path: String::new(),
            }
        }
    }
}

/// A complete selector definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectorDefinition {
    pub sign: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_folder_scope")]
    pub folder_scope: String,
    pub rules: Vec<SelectorRule>,
    pub match_mode: MatchMode,
    /// Custom expression string (only used when match_mode is Custom).
    #[serde(default)]
    pub custom_expression: String,
    #[serde(default)]
    pub skills: Vec<SelectorSkillRef>,
    #[serde(default)]
    pub flocks: Vec<SelectorSkillRef>,
    /// Repository references. When this selector matches, all flocks and skills
    /// from these repos will be fetched.
    #[serde(default)]
    pub repos: Vec<SelectorRepo>,
    /// Priority (higher value = higher priority). When multiple selectors
    /// contribute conflicting skills, the selector with the higher priority wins.
    #[serde(default)]
    pub priority: i32,
    /// How many times this selector has been matched by `savhub apply`.
    #[serde(default)]
    pub match_count: i64,
}

fn default_folder_scope() -> String {
    ".".to_string()
}

/// Persistent store for all selector definitions.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SelectorsStore {
    pub version: u8,
    pub selectors: Vec<SelectorDefinition>,
}

// ---------------------------------------------------------------------------
// Official selectors
// ---------------------------------------------------------------------------

/// A single official selector entry: the selector definition plus metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfficialSelectorEntry {
    pub selector: SelectorDefinition,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Store for official selectors synced from the server.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OfficialSelectorsStore {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    pub selectors: Vec<OfficialSelectorEntry>,
}

/// User preference overlay — tracks which selectors (official or custom) are disabled.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SelectorPrefs {
    pub version: u8,
    pub disabled: std::collections::BTreeSet<String>,
}

/// Returns `true` when `sign` belongs to an official selector.
pub fn is_official_selector(sign: &str) -> bool {
    sign.starts_with("official:")
}

// ---------------------------------------------------------------------------
// Expression builder & evaluation
// ---------------------------------------------------------------------------

impl SelectorDefinition {
    /// Build the effective rule expression from the match mode.
    pub fn build_expression(&self) -> Result<RuleExpression> {
        match self.match_mode {
            MatchMode::AllMatch => {
                let operands: Vec<RuleExpression> = (0..self.rules.len())
                    .map(|i| RuleExpression::Check { index: i })
                    .collect();
                if operands.is_empty() {
                    bail!("no rules defined");
                }
                Ok(if operands.len() == 1 {
                    operands.into_iter().next().unwrap()
                } else {
                    RuleExpression::And { operands }
                })
            }
            MatchMode::AnyMatch => {
                let operands: Vec<RuleExpression> = (0..self.rules.len())
                    .map(|i| RuleExpression::Check { index: i })
                    .collect();
                if operands.is_empty() {
                    bail!("no rules defined");
                }
                Ok(if operands.len() == 1 {
                    operands.into_iter().next().unwrap()
                } else {
                    RuleExpression::Or { operands }
                })
            }
            MatchMode::Custom => parse_expression(&self.custom_expression, self.rules.len()),
        }
    }

    /// Evaluate this selector against a project root directory.
    pub fn evaluate(&self, project_root: &Path) -> bool {
        let Ok(expr) = self.build_expression() else {
            return false;
        };
        let base = if self.folder_scope == "." || self.folder_scope.is_empty() {
            project_root.to_path_buf()
        } else {
            project_root.join(&self.folder_scope)
        };
        expr.evaluate(&base, &self.rules)
    }

    /// Generate a human-readable expression string.
    pub fn display_expression(&self) -> String {
        match self.match_mode {
            MatchMode::AllMatch => (1..=self.rules.len())
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" && "),
            MatchMode::AnyMatch => (1..=self.rules.len())
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" || "),
            MatchMode::Custom => self.custom_expression.clone(),
        }
    }
}

impl RuleExpression {
    /// Evaluate the expression tree against a base directory.
    pub fn evaluate(&self, base: &Path, rules: &[SelectorRule]) -> bool {
        match self {
            RuleExpression::Check { index } => {
                rules.get(*index).is_some_and(|rule| rule.evaluate(base))
            }
            RuleExpression::And { operands } => operands.iter().all(|op| op.evaluate(base, rules)),
            RuleExpression::Or { operands } => operands.iter().any(|op| op.evaluate(base, rules)),
            RuleExpression::Not { operand } => !operand.evaluate(base, rules),
        }
    }

    /// Convert the expression tree to a human-readable string with 1-based rule numbers.
    pub fn to_display_string(&self) -> String {
        self.fmt_inner(false)
    }

    fn fmt_inner(&self, needs_parens: bool) -> String {
        match self {
            RuleExpression::Check { index } => format!("{}", index + 1),
            RuleExpression::And { operands } => {
                let inner = operands
                    .iter()
                    .map(|op| op.fmt_inner(matches!(op, RuleExpression::Or { .. })))
                    .collect::<Vec<_>>()
                    .join(" && ");
                if needs_parens {
                    format!("({inner})")
                } else {
                    inner
                }
            }
            RuleExpression::Or { operands } => {
                let inner = operands
                    .iter()
                    .map(|op| op.fmt_inner(false))
                    .collect::<Vec<_>>()
                    .join(" || ");
                if needs_parens {
                    format!("({inner})")
                } else {
                    inner
                }
            }
            RuleExpression::Not { operand } => {
                let wrap = matches!(
                    **operand,
                    RuleExpression::And { .. } | RuleExpression::Or { .. }
                );
                format!("!{}", operand.fmt_inner(wrap))
            }
        }
    }
}

impl SelectorRule {
    /// Evaluate a single rule against a base directory.
    pub fn evaluate(&self, base: &Path) -> bool {
        match self {
            SelectorRule::FileExists { path } => base.join(path).is_file(),
            SelectorRule::FolderExists { path } => base.join(path).is_dir(),
            SelectorRule::GlobMatch { pattern } => glob_any_match(base, pattern),
            SelectorRule::FileContains { path, contains } => {
                std::fs::read_to_string(base.join(path))
                    .map(|content| content.contains(contains.as_str()))
                    .unwrap_or(false)
            }
            SelectorRule::FileRegex { path, pattern } => {
                let Ok(re) = regex::Regex::new(pattern) else {
                    return false;
                };
                std::fs::read_to_string(base.join(path))
                    .map(|content| re.is_match(&content))
                    .unwrap_or(false)
            }
            SelectorRule::EnvVarSet { name } => {
                std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false)
            }
            SelectorRule::CommandExits { command } => {
                #[cfg(target_os = "windows")]
                {
                    std::process::Command::new("cmd")
                        .args(["/C", command])
                        .current_dir(base)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                }
                #[cfg(not(target_os = "windows"))]
                {
                    std::process::Command::new("sh")
                        .args(["-c", command])
                        .current_dir(base)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                }
            }
        }
    }

    /// Human-readable display string.
    pub fn display(&self) -> String {
        match self {
            SelectorRule::FileExists { path } => format!("File: {path}"),
            SelectorRule::FolderExists { path } => format!("Folder: {path}"),
            SelectorRule::GlobMatch { pattern } => format!("Glob: {pattern}"),
            SelectorRule::FileContains { path, contains } => {
                format!("Contains: {path} → \"{contains}\"")
            }
            SelectorRule::FileRegex { path, pattern } => {
                format!("Regex: {path} → /{pattern}/")
            }
            SelectorRule::EnvVarSet { name } => format!("Env: ${name}"),
            SelectorRule::CommandExits { command } => format!("Cmd: {command}"),
        }
    }

    /// Short kind string for form selectors.
    pub fn kind_str(&self) -> &'static str {
        match self {
            SelectorRule::FileExists { .. } => "file_exists",
            SelectorRule::FolderExists { .. } => "folder_exists",
            SelectorRule::GlobMatch { .. } => "glob_match",
            SelectorRule::FileContains { .. } => "file_contains",
            SelectorRule::FileRegex { .. } => "file_regex",
            SelectorRule::EnvVarSet { .. } => "env_var_set",
            SelectorRule::CommandExits { .. } => "command_exits",
        }
    }
}

/// Check if any file under `base` matches the given glob pattern.
///
/// Supports `*` (any chars in filename), `?` (single char), and `**` (recursive dirs).
fn glob_any_match(base: &Path, pattern: &str) -> bool {
    use walkdir::WalkDir;

    // Normalise pattern separators to /
    let pat = pattern.replace('\\', "/");

    for entry in WalkDir::new(base).max_depth(10).into_iter().flatten() {
        let Ok(rel) = entry.path().strip_prefix(base) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if glob_pattern_matches(&pat, &rel_str) {
            return true;
        }
    }
    false
}

/// Simple glob matching: `*` matches non-`/` chars, `**` matches anything, `?` matches one char.
fn glob_pattern_matches(pattern: &str, text: &str) -> bool {
    glob_match_recursive(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_recursive(pat: &[u8], txt: &[u8]) -> bool {
    if pat.is_empty() {
        return txt.is_empty();
    }

    // Handle ** (matches any path segments)
    if pat.starts_with(b"**") {
        let rest = if pat.len() > 2 && pat[2] == b'/' {
            &pat[3..]
        } else {
            &pat[2..]
        };
        // Try matching rest against every suffix of txt
        for i in 0..=txt.len() {
            if glob_match_recursive(rest, &txt[i..]) {
                return true;
            }
        }
        return false;
    }

    if txt.is_empty() {
        return false;
    }

    match pat[0] {
        b'*' => {
            // * matches zero or more non-/ characters
            for i in 0..=txt.len() {
                if i > 0 && txt[i - 1] == b'/' {
                    break;
                }
                if glob_match_recursive(&pat[1..], &txt[i..]) {
                    return true;
                }
            }
            false
        }
        b'?' => {
            if txt[0] != b'/' {
                glob_match_recursive(&pat[1..], &txt[1..])
            } else {
                false
            }
        }
        c => {
            if c == txt[0] {
                glob_match_recursive(&pat[1..], &txt[1..])
            } else {
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Expression parser
// ---------------------------------------------------------------------------

/// Supported expression tokens.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(usize),
    And,
    Or,
    Not,
    LParen,
    RParen,
}

fn tokenize(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            '!' => {
                tokens.push(Token::Not);
                chars.next();
            }
            '&' => {
                chars.next();
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::And);
                } else {
                    bail!("expected '&&', got single '&'");
                }
            }
            '|' => {
                chars.next();
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Or);
                } else {
                    bail!("expected '||', got single '|'");
                }
            }
            '0'..='9' => {
                let mut num = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        num.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let n: usize = num.parse().context("invalid number")?;
                tokens.push(Token::Number(n));
            }
            other => bail!("unexpected character: '{other}'"),
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    max_index: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>, max_index: usize) -> Self {
        Self {
            tokens,
            pos: 0,
            max_index,
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        Some(token)
    }

    fn expect(&mut self, expected: &Token) -> Result<()> {
        let token = self.advance().context("unexpected end of expression")?;
        if &token != expected {
            bail!("expected {expected:?}, got {token:?}");
        }
        Ok(())
    }

    /// expr = or_expr
    fn parse_expr(&mut self) -> Result<RuleExpression> {
        self.parse_or()
    }

    /// or_expr = and_expr ("||" and_expr)*
    fn parse_or(&mut self) -> Result<RuleExpression> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Token::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = match left {
                RuleExpression::Or { mut operands } => {
                    operands.push(right);
                    RuleExpression::Or { operands }
                }
                _ => RuleExpression::Or {
                    operands: vec![left, right],
                },
            };
        }
        Ok(left)
    }

    /// and_expr = unary ("&&" unary)*
    fn parse_and(&mut self) -> Result<RuleExpression> {
        let mut left = self.parse_unary()?;
        while self.peek() == Some(&Token::And) {
            self.advance();
            let right = self.parse_unary()?;
            left = match left {
                RuleExpression::And { mut operands } => {
                    operands.push(right);
                    RuleExpression::And { operands }
                }
                _ => RuleExpression::And {
                    operands: vec![left, right],
                },
            };
        }
        Ok(left)
    }

    /// unary = "!" unary | primary
    fn parse_unary(&mut self) -> Result<RuleExpression> {
        if self.peek() == Some(&Token::Not) {
            self.advance();
            let operand = self.parse_unary()?;
            Ok(RuleExpression::Not {
                operand: Box::new(operand),
            })
        } else {
            self.parse_primary()
        }
    }

    /// primary = NUMBER | "(" expr ")"
    fn parse_primary(&mut self) -> Result<RuleExpression> {
        match self.advance() {
            Some(Token::Number(n)) => {
                if n == 0 || n > self.max_index {
                    bail!("rule number {n} out of range (1..={})", self.max_index);
                }
                Ok(RuleExpression::Check { index: n - 1 })
            }
            Some(Token::LParen) => {
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            Some(other) => bail!("unexpected token: {other:?}"),
            None => bail!("unexpected end of expression"),
        }
    }
}

/// Parse an expression string like `(1 && 2) || !3` into a `RuleExpression` tree.
///
/// Rule numbers are 1-based. `max_rules` is the total number of available rules.
pub fn parse_expression(input: &str, max_rules: usize) -> Result<RuleExpression> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("expression is empty");
    }
    let tokens = tokenize(trimmed)?;
    if tokens.is_empty() {
        bail!("expression is empty");
    }
    let mut parser = Parser::new(tokens, max_rules);
    let expr = parser.parse_expr()?;
    if parser.pos < parser.tokens.len() {
        bail!("unexpected tokens after expression");
    }
    Ok(expr)
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

fn selectors_dir() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("selectors"))
}

fn custom_selectors_path() -> Result<PathBuf> {
    Ok(selectors_dir()?.join("custom.json"))
}

pub fn read_selectors_store() -> Result<SelectorsStore> {
    let path = custom_selectors_path()?;
    if let Ok(raw) = fs::read_to_string(&path) {
        let store: SelectorsStore = serde_json::from_str(&raw)
            .with_context(|| format!("invalid selectors at {}", path.display()))?;
        return Ok(store);
    }
    Ok(SelectorsStore {
        version: 1,
        selectors: Vec::new(),
    })
}

pub fn write_selectors_store(store: &SelectorsStore) -> Result<()> {
    let path = custom_selectors_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(store)?;
    fs::write(&path, format!("{payload}\n"))?;
    let _ = crate::pilot::notify_config_changed();
    Ok(())
}

/// Generate a unique ID for a new selector.
pub fn generate_selector_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("det-{ts:x}")
}

/// Deduplicate skills, flocks, and repos in a selector before saving.
fn dedup_selector(mut d: SelectorDefinition) -> SelectorDefinition {
    let mut seen = std::collections::BTreeSet::new();
    d.skills.retain(|s| seen.insert(s.clone()));
    let mut seen = std::collections::BTreeSet::new();
    d.flocks.retain(|s| seen.insert(s.clone()));
    let mut seen = std::collections::BTreeSet::new();
    d.repos.retain(|r| seen.insert(r.clone()));
    d
}

pub fn create_selector(selector: SelectorDefinition) -> Result<()> {
    let mut store = read_selectors_store()?;
    if store.selectors.iter().any(|d| d.sign == selector.sign) {
        bail!("selector with id '{}' already exists", selector.sign);
    }
    store.selectors.push(dedup_selector(selector));
    write_selectors_store(&store)
}

pub fn update_selector(selector: SelectorDefinition) -> Result<()> {
    let mut store = read_selectors_store()?;
    if let Some(existing) = store.selectors.iter_mut().find(|d| d.sign == selector.sign) {
        *existing = dedup_selector(selector);
    } else {
        bail!("selector '{}' not found", selector.sign);
    }
    write_selectors_store(&store)
}

pub fn set_selector_enabled(sign: &str, enabled: bool) -> Result<()> {
    let mut prefs = read_selector_prefs()?;
    if enabled {
        prefs.disabled.remove(sign);
    } else {
        prefs.disabled.insert(sign.to_string());
    }
    write_selector_prefs(&prefs)
}

pub fn delete_selector(id: &str) -> Result<()> {
    let mut store = read_selectors_store()?;
    let before = store.selectors.len();
    store.selectors.retain(|d| d.sign != id);
    if store.selectors.len() == before {
        bail!("selector '{id}' not found");
    }
    write_selectors_store(&store)
}

// ---------------------------------------------------------------------------
// Official selector persistence & operations
// ---------------------------------------------------------------------------

fn official_selectors_path() -> Result<PathBuf> {
    Ok(selectors_dir()?.join("official.json"))
}

fn selector_prefs_path() -> Result<PathBuf> {
    Ok(selectors_dir()?.join("prefs.json"))
}

/// Read the official selectors store (empty if file does not exist yet).
pub fn read_official_selectors_store() -> Result<OfficialSelectorsStore> {
    let path = official_selectors_path()?;
    if let Ok(raw) = fs::read_to_string(&path) {
        let store: OfficialSelectorsStore = serde_json::from_str(&raw)
            .with_context(|| format!("invalid official selectors at {}", path.display()))?;
        eprintln!(
            "[savhub] read official store: {} selector(s) from {}",
            store.selectors.len(),
            path.display()
        );
        return Ok(store);
    }
    eprintln!("[savhub] official store not found at {}", path.display());
    Ok(OfficialSelectorsStore::default())
}

/// Write the official selectors store.
pub fn write_official_selectors_store(store: &OfficialSelectorsStore) -> Result<()> {
    let path = official_selectors_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(store)?;
    fs::write(&path, format!("{payload}\n"))?;
    let _ = crate::pilot::notify_config_changed();
    Ok(())
}

/// Read selector preferences (empty if file does not exist).
pub fn read_selector_prefs() -> Result<SelectorPrefs> {
    let path = selector_prefs_path()?;
    if let Ok(raw) = fs::read_to_string(&path) {
        let prefs: SelectorPrefs = serde_json::from_str(&raw)
            .with_context(|| format!("invalid selector prefs at {}", path.display()))?;
        return Ok(prefs);
    }
    Ok(SelectorPrefs::default())
}

/// Write selector preferences.
pub fn write_selector_prefs(prefs: &SelectorPrefs) -> Result<()> {
    let path = selector_prefs_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(prefs)?;
    fs::write(&path, format!("{payload}\n"))?;
    let _ = crate::pilot::notify_config_changed();
    Ok(())
}

/// Check whether a selector is enabled (i.e. not in the disabled set).
pub fn is_selector_enabled(sign: &str) -> bool {
    read_selector_prefs()
        .map(|p| !p.disabled.contains(sign))
        .unwrap_or(true)
}

/// Enable or disable ALL official selectors at once.
pub fn set_all_official_selectors_enabled(enabled: bool) -> Result<()> {
    let mut prefs = read_selector_prefs()?;
    if enabled {
        // Only remove official signs from disabled set, keep custom disabled intact.
        prefs.disabled.retain(|s| !is_official_selector(s));
    } else {
        let store = read_official_selectors_store()?;
        for entry in &store.selectors {
            prefs.disabled.insert(entry.selector.sign.clone());
        }
    }
    write_selector_prefs(&prefs)
}

pub fn set_all_custom_selectors_enabled(enabled: bool) -> Result<()> {
    let mut prefs = read_selector_prefs()?;
    if enabled {
        // Only remove custom signs from disabled set, keep official disabled intact.
        prefs.disabled.retain(|s| is_official_selector(s));
    } else {
        let store = read_selectors_store()?;
        for entry in &store.selectors {
            prefs.disabled.insert(entry.sign.clone());
        }
    }
    write_selector_prefs(&prefs)
}

/// Clone an official selector as a new custom selector.
///
/// Returns the cloned definition (already saved to the custom store).
pub fn clone_official_as_custom(sign: &str) -> Result<SelectorDefinition> {
    let official = read_official_selectors_store()?;
    let entry = official
        .selectors
        .iter()
        .find(|e| e.selector.sign == sign)
        .with_context(|| format!("official selector '{sign}' not found"))?;

    let mut cloned = entry.selector.clone();
    cloned.sign = generate_selector_id();
    cloned.name = format!("{} (copy)", cloned.name);
    cloned.match_count = 0;

    create_selector(cloned.clone())?;
    Ok(cloned)
}

/// Convert an official selector JSON value into an
/// `OfficialSelectorEntry`.
pub fn selector_value_to_entry(value: &serde_json::Value) -> Result<OfficialSelectorEntry> {
    // Extract tags before parsing the selector (tags are not part of SelectorDefinition)
    let tags: Vec<String> = value
        .get("tags")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // Parse the selector definition from the same value.
    // We strip fields unknown to SelectorDefinition so deserialization succeeds.
    let selector: SelectorDefinition = serde_json::from_value(value.clone())
        .context("failed to parse selector into SelectorDefinition")?;

    Ok(OfficialSelectorEntry { selector, tags })
}

/// Sync official selectors from the server API.
///
/// Returns `Ok(true)` if the store was updated, `Ok(false)` if unchanged (304).
pub fn sync_official_selectors(api_base: &str) -> Result<bool> {
    let current = read_official_selectors_store()?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let url = format!("{}/selectors/official", api_base.trim_end_matches('/'));
    eprintln!("[savhub] GET {url}");
    let mut req = client.get(&url);
    if let Some(etag) = &current.etag {
        eprintln!("[savhub]   If-None-Match: {etag}");
        req = req.header("If-None-Match", etag.as_str());
    }

    let resp = req
        .send()
        .inspect_err(|e| eprintln!("[savhub]   request error: {e}"))?;
    eprintln!(
        "[savhub]   response: {} content-type={:?}",
        resp.status(),
        resp.headers().get("content-type")
    );
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(false);
    }
    if !resp.status().is_success() {
        bail!("official selectors API returned {}", resp.status());
    }

    // Guard against SPA catch-all returning HTML instead of JSON.
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("application/json") {
        bail!(
            "official selectors API returned unexpected Content-Type: {content_type} (is the server deployed?)"
        );
    }

    let body: serde_json::Value = resp.json()?;
    let etag = body
        .get("etag")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let selectors = body
        .get("selectors")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut entries = Vec::new();
    for selector in &selectors {
        match selector_value_to_entry(selector) {
            Ok(entry) => entries.push(entry),
            Err(err) => {
                eprintln!("[savhub] skipping invalid selector: {err}");
            }
        }
    }

    eprintln!(
        "[savhub]   parsed {} official selector(s) from response",
        entries.len()
    );
    let store = OfficialSelectorsStore {
        version: 1,
        last_synced_at: Some(chrono::Utc::now().to_rfc3339()),
        etag,
        selectors: entries,
    };
    write_official_selectors_store(&store)?;
    eprintln!(
        "[savhub]   wrote official store to {}",
        official_selectors_path()?.display()
    );
    Ok(true)
}

// ---------------------------------------------------------------------------
// Custom selectors cloud sync
// ---------------------------------------------------------------------------

/// Push local custom selectors to the server.
pub fn push_custom_selectors(api_base: &str, token: &str) -> Result<()> {
    let store = read_selectors_store()?;
    let selectors: Vec<serde_json::Value> = store
        .selectors
        .iter()
        .filter_map(|s| serde_json::to_value(s).ok())
        .collect();

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let url = format!("{}/me/selectors/custom", api_base.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(&serde_json::json!({
            "selectors": selectors,
            "version": store.version,
        }))
        .send()?;

    if !resp.status().is_success() {
        bail!("push custom selectors failed: HTTP {}", resp.status());
    }
    Ok(())
}

/// Pull custom selectors from the server. Returns `None` if the server has none.
pub fn pull_custom_selectors(api_base: &str, token: &str) -> Result<Option<SelectorsStore>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let url = format!("{}/me/selectors/custom", api_base.trim_end_matches('/'));
    let resp = client.get(&url).bearer_auth(token).send()?;

    if !resp.status().is_success() {
        bail!("pull custom selectors failed: HTTP {}", resp.status());
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("application/json") {
        bail!("pull custom selectors: unexpected Content-Type: {content_type}");
    }

    let body: serde_json::Value = resp.json()?;
    let selectors_arr = body
        .get("selectors")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if selectors_arr.is_empty() {
        return Ok(None);
    }

    let mut selectors = Vec::new();
    for val in &selectors_arr {
        match serde_json::from_value::<SelectorDefinition>(val.clone()) {
            Ok(def) => selectors.push(def),
            Err(e) => eprintln!("[savhub] skipping invalid remote selector: {e}"),
        }
    }

    let version = body.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u8;

    Ok(Some(SelectorsStore { version, selectors }))
}

/// Conflict when merging selectors: same sign, different content.
#[derive(Debug, Clone)]
pub struct SelectorConflict {
    pub sign: String,
    pub name: String,
}

/// Result of merging remote selectors into local store.
#[derive(Debug)]
pub struct MergeResult {
    pub added: usize,
    pub conflicts: Vec<SelectorConflict>,
}

/// Merge remote selectors into local store, write the result, and return merge info.
///
/// - Same sign + same content → skip
/// - Same sign + different content → keep local, report conflict
/// - Only remote → add to local
/// - Only local → keep
pub fn merge_and_apply(remote: SelectorsStore) -> Result<MergeResult> {
    let mut local = read_selectors_store()?;
    let mut added = 0;
    let mut conflicts = Vec::new();

    for remote_sel in &remote.selectors {
        if let Some(local_sel) = local.selectors.iter().find(|d| d.sign == remote_sel.sign) {
            if local_sel != remote_sel {
                conflicts.push(SelectorConflict {
                    sign: remote_sel.sign.clone(),
                    name: remote_sel.name.clone(),
                });
            }
        } else {
            local.selectors.push(remote_sel.clone());
            added += 1;
        }
    }

    if added > 0 {
        write_selectors_store(&local)?;
    }

    Ok(MergeResult { added, conflicts })
}

/// Update match counts after `savhub apply`:
/// - Increment for selectors that matched
/// - Decrement for selectors that previously matched but no longer do
pub fn update_match_counts(matched_names: &[String], unmatched_names: &[String]) -> Result<()> {
    let mut store = read_selectors_store()?;
    let mut changed = false;
    for selector in &mut store.selectors {
        if matched_names.iter().any(|n| n == &selector.name) {
            selector.match_count += 1;
            changed = true;
        } else if unmatched_names.iter().any(|n| n == &selector.name) {
            selector.match_count = (selector.match_count - 1).max(0);
            changed = true;
        }
    }
    if changed {
        write_selectors_store(&store)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Selector execution engine
// ---------------------------------------------------------------------------

/// A matched selector with its collected skills.
#[derive(Debug, Clone)]
pub struct SelectorMatch {
    pub selector: SelectorDefinition,
    pub skills: Vec<SelectorSkillRef>,
    pub flocks: Vec<SelectorSkillRef>,
    pub repos: Vec<SelectorRepo>,
}

/// Result of running all selectors against a project.
#[derive(Debug, Clone)]
pub struct SelectorRunResult {
    /// Selectors that matched, sorted by priority (highest first).
    pub matched: Vec<SelectorMatch>,
    /// Merged skills with priority-based conflict resolution.
    /// Higher-priority selectors' skills take precedence.
    pub skills: Vec<SelectorSkillRef>,
    /// Merged flocks from all matched selectors.
    pub flocks: Vec<SelectorSkillRef>,
    /// Merged repos from all matched selectors.
    pub repos: Vec<SelectorRepo>,
}

/// Run all selectors against a project directory.
///
/// Selectors are evaluated in priority order (highest first).
/// When multiple selectors contribute a skill with the same slug,
/// the higher-priority selector wins.
pub fn run_selectors(project_root: &Path) -> Result<SelectorRunResult> {
    // Merge official selectors with custom selectors, applying user prefs.
    let official_store = read_official_selectors_store()?;
    let prefs = read_selector_prefs()?;
    let custom_store = read_selectors_store()?;

    let mut all_selectors: Vec<SelectorDefinition> = Vec::new();
    for entry in &official_store.selectors {
        all_selectors.push(entry.selector.clone());
    }
    all_selectors.extend(custom_store.selectors.clone());

    let mut matched: Vec<SelectorMatch> = Vec::new();

    for selector in &all_selectors {
        if prefs.disabled.contains(&selector.sign) {
            continue;
        }
        if selector.evaluate(project_root) {
            // Expand repos into flocks: look up all flock refs for each repo
            let mut expanded_flocks = selector.flocks.clone();
            for repo in &selector.repos {
                if let Ok(repo_flocks) = crate::registry::list_repo_flock_refs(&repo.git_url) {
                    for flock_ref in repo_flocks {
                        if !expanded_flocks.contains(&flock_ref) {
                            expanded_flocks.push(flock_ref);
                        }
                    }
                }
            }
            matched.push(SelectorMatch {
                selector: selector.clone(),
                skills: selector.skills.clone(),
                flocks: expanded_flocks,
                repos: selector.repos.clone(),
            });
        }
    }

    // Sort by priority descending (higher priority first)
    matched.sort_by(|a, b| b.selector.priority.cmp(&a.selector.priority));

    // Higher-priority selector's items come first and take precedence over duplicates.
    let skills = dedup_concat(matched.iter().flat_map(|m| m.skills.iter()), |s| s.clone());
    let flocks = dedup_concat(matched.iter().flat_map(|m| m.flocks.iter()), |f| f.clone());
    let repos = dedup_concat(matched.iter().flat_map(|m| m.repos.iter()), |r| {
        r.git_url.clone()
    });

    Ok(SelectorRunResult {
        matched,
        skills,
        flocks,
        repos,
    })
}

/// Walk `items` in order, keeping the first occurrence of each unique key.
/// Used by `run_selectors` to merge skills/flocks/repos across matched
/// selectors while preserving priority order.
fn dedup_concat<'a, T, K, I, F>(items: I, key_fn: F) -> Vec<T>
where
    T: Clone + 'a,
    K: Ord,
    I: Iterator<Item = &'a T>,
    F: Fn(&T) -> K,
{
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(key_fn(item)) {
            out.push(item.clone());
        }
    }
    out
}

#[allow(dead_code)]
fn seed_default_selectors(store: &mut SelectorsStore) {
    let defaults = vec![
        // ── Language-level selectors ─────────────────────────
        SelectorDefinition {
            sign: "builtin-rust-project".to_string(),
            name: "Rust Project".to_string(),
            description: "Detects Rust projects by the presence of Cargo.toml.".to_string(),
            folder_scope: ".".to_string(),
            rules: vec![SelectorRule::FileExists {
                path: "Cargo.toml".to_string(),
            }],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 10,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-python-project".to_string(),
            name: "Python Project".to_string(),
            description: "Detects Python projects by pyproject.toml or requirements.txt."
                .to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "pyproject.toml".to_string(),
                },
                SelectorRule::FileExists {
                    path: "requirements.txt".to_string(),
                },
                SelectorRule::FileExists {
                    path: "setup.py".to_string(),
                },
            ],
            match_mode: MatchMode::AnyMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 10,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-go-project".to_string(),
            name: "Go Project".to_string(),
            description: "Detects Go projects by the presence of go.mod.".to_string(),
            folder_scope: ".".to_string(),
            rules: vec![SelectorRule::FileExists {
                path: "go.mod".to_string(),
            }],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 10,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-java-project".to_string(),
            name: "Java / Kotlin Project".to_string(),
            description: "Detects JVM projects via pom.xml or build.gradle.".to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "pom.xml".to_string(),
                },
                SelectorRule::FileExists {
                    path: "build.gradle".to_string(),
                },
                SelectorRule::FileExists {
                    path: "build.gradle.kts".to_string(),
                },
            ],
            match_mode: MatchMode::AnyMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 10,
            match_count: 0,
        },
        // ── Rust framework selectors ─────────────────────────
        SelectorDefinition {
            sign: "builtin-salvo-project".to_string(),
            name: "Salvo Web Framework".to_string(),
            description: "Detects Rust projects using the Salvo web framework.".to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "Cargo.toml".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "Cargo.toml".to_string(),
                    pattern: r#"salvo\s*="#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![SelectorSkillRef {
                repo: "github.com/salvo-rs/salvo-skills".to_string(),
                path: "salvo-skills".to_string(),
            }],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-actix-project".to_string(),
            name: "Actix Web Framework".to_string(),
            description: "Detects Rust projects using the Actix-web framework.".to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "Cargo.toml".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "Cargo.toml".to_string(),
                    pattern: r#"actix-web\s*="#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-axum-project".to_string(),
            name: "Axum Web Framework".to_string(),
            description: "Detects Rust projects using the Axum web framework.".to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "Cargo.toml".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "Cargo.toml".to_string(),
                    pattern: r#"axum\s*="#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-dioxus-project".to_string(),
            name: "Dioxus Framework".to_string(),
            description: "Detects Rust projects using the Dioxus UI framework.".to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "Cargo.toml".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "Cargo.toml".to_string(),
                    pattern: r#"dioxus\s*="#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-makepad-project".to_string(),
            name: "Makepad Project".to_string(),
            description: "Detects Makepad projects by checking Cargo.toml for makepad dependency."
                .to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "Cargo.toml".to_string(),
                },
                SelectorRule::FileContains {
                    path: "Cargo.toml".to_string(),
                    contains: "makepad".to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![SelectorRepo::from_url(
                "github.com/ZhangHanDong/makepad-skills",
            )],

            priority: 20,
            match_count: 0,
        },
        // ── JS/TS framework selectors ────────────────────────
        SelectorDefinition {
            sign: "builtin-web-frontend".to_string(),
            name: "Web Frontend (Node/TS)".to_string(),
            description: "Detects Node.js or TypeScript frontend projects.".to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "package.json".to_string(),
                },
                SelectorRule::FileExists {
                    path: "tsconfig.json".to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 10,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-react-project".to_string(),
            name: "React".to_string(),
            description: "Detects React projects by checking package.json for react dependency."
                .to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "package.json".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "package.json".to_string(),
                    pattern: r#""react"\s*:"#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-vue-project".to_string(),
            name: "Vue".to_string(),
            description: "Detects Vue.js projects by checking package.json for vue dependency."
                .to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "package.json".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "package.json".to_string(),
                    pattern: r#""vue"\s*:"#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-angular-project".to_string(),
            name: "Angular".to_string(),
            description: "Detects Angular projects by checking package.json for @angular/core."
                .to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "package.json".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "package.json".to_string(),
                    pattern: r#""@angular/core"\s*:"#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-svelte-project".to_string(),
            name: "Svelte".to_string(),
            description: "Detects Svelte projects by checking package.json for svelte dependency."
                .to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "package.json".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "package.json".to_string(),
                    pattern: r#""svelte"\s*:"#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-nextjs-project".to_string(),
            name: "Next.js".to_string(),
            description: "Detects Next.js projects by checking package.json for next dependency."
                .to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "package.json".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "package.json".to_string(),
                    pattern: r#""next"\s*:"#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        SelectorDefinition {
            sign: "builtin-nuxt-project".to_string(),
            name: "Nuxt".to_string(),
            description: "Detects Nuxt projects by checking package.json for nuxt dependency."
                .to_string(),
            folder_scope: ".".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "package.json".to_string(),
                },
                SelectorRule::FileRegex {
                    path: "package.json".to_string(),
                    pattern: r#""nuxt"\s*:"#.to_string(),
                },
            ],
            match_mode: MatchMode::AllMatch,
            custom_expression: String::new(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
        // ── Monorepo ─────────────────────────────────────────
        SelectorDefinition {
            sign: "builtin-monorepo-web".to_string(),
            name: "Monorepo Web App".to_string(),
            description: "Scopes detection to a workspace folder inside a monorepo.".to_string(),
            folder_scope: "apps/web".to_string(),
            rules: vec![
                SelectorRule::FileExists {
                    path: "package.json".to_string(),
                },
                SelectorRule::FileExists {
                    path: "vite.config.ts".to_string(),
                },
                SelectorRule::FileExists {
                    path: "../pnpm-workspace.yaml".to_string(),
                },
            ],
            match_mode: MatchMode::Custom,
            custom_expression: "(1 && 2) || 3".to_string(),

            skills: vec![],
            flocks: vec![],
            repos: vec![],

            priority: 20,
            match_count: 0,
        },
    ];
    for selector in defaults {
        if !store.selectors.iter().any(|d| d.sign == selector.sign) {
            store.selectors.push(selector);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_match() {
        let expr = parse_expression("1 && 2 && 3", 3).unwrap();
        assert_eq!(
            expr,
            RuleExpression::And {
                operands: vec![
                    RuleExpression::Check { index: 0 },
                    RuleExpression::Check { index: 1 },
                    RuleExpression::Check { index: 2 },
                ]
            }
        );
    }

    #[test]
    fn parse_any_match() {
        let expr = parse_expression("1 || 2 || 3", 3).unwrap();
        assert_eq!(
            expr,
            RuleExpression::Or {
                operands: vec![
                    RuleExpression::Check { index: 0 },
                    RuleExpression::Check { index: 1 },
                    RuleExpression::Check { index: 2 },
                ]
            }
        );
    }

    #[test]
    fn parse_mixed_with_parens() {
        let expr = parse_expression("(1 && 2) || !3", 3).unwrap();
        assert_eq!(
            expr,
            RuleExpression::Or {
                operands: vec![
                    RuleExpression::And {
                        operands: vec![
                            RuleExpression::Check { index: 0 },
                            RuleExpression::Check { index: 1 },
                        ]
                    },
                    RuleExpression::Not {
                        operand: Box::new(RuleExpression::Check { index: 2 }),
                    },
                ]
            }
        );
    }

    #[test]
    fn parse_nested() {
        let expr = parse_expression("(1 || 2) && (3 || !4)", 4).unwrap();
        assert_eq!(
            expr,
            RuleExpression::And {
                operands: vec![
                    RuleExpression::Or {
                        operands: vec![
                            RuleExpression::Check { index: 0 },
                            RuleExpression::Check { index: 1 },
                        ]
                    },
                    RuleExpression::Or {
                        operands: vec![
                            RuleExpression::Check { index: 2 },
                            RuleExpression::Not {
                                operand: Box::new(RuleExpression::Check { index: 3 }),
                            },
                        ]
                    },
                ]
            }
        );
    }

    #[test]
    fn parse_single_rule() {
        let expr = parse_expression("1", 1).unwrap();
        assert_eq!(expr, RuleExpression::Check { index: 0 });
    }

    #[test]
    fn parse_out_of_range() {
        assert!(parse_expression("5", 3).is_err());
        assert!(parse_expression("0", 3).is_err());
    }

    #[test]
    fn display_round_trip() {
        // AND binds tighter than OR, so (1 && 2) || !3 displays without redundant parens.
        let expr = parse_expression("(1 && 2) || !3", 3).unwrap();
        let display = expr.to_display_string();
        assert_eq!(display, "1 && 2 || !3");

        // Re-parsing the display string should produce the same tree.
        let expr2 = parse_expression(&display, 3).unwrap();
        assert_eq!(expr, expr2);
    }

    #[test]
    fn display_preserves_needed_parens() {
        // OR inside AND needs parens: 1 && (2 || 3)
        let expr = parse_expression("1 && (2 || 3)", 3).unwrap();
        let display = expr.to_display_string();
        assert_eq!(display, "1 && (2 || 3)");

        let expr2 = parse_expression(&display, 3).unwrap();
        assert_eq!(expr, expr2);
    }

    #[test]
    fn display_simple_and() {
        let expr = RuleExpression::And {
            operands: vec![
                RuleExpression::Check { index: 0 },
                RuleExpression::Check { index: 1 },
                RuleExpression::Check { index: 2 },
            ],
        };
        assert_eq!(expr.to_display_string(), "1 && 2 && 3");
    }
}
