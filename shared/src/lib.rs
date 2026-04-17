mod bundle_meta;
mod catalog;
mod client;

pub use bundle_meta::*;
pub use catalog::*;
use chrono::{DateTime, Utc};
pub use client::*;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityStatus {
    /// Passed static scan only (AI not enabled).
    Checked,
    /// AI scan in progress (atomically claimed from `checked`).
    Scanning,
    /// Passed both static and AI scans.
    Verified,
    Suspicious,
    Malicious,
    /// Unknown or not yet scanned (default, catches unrecognized values).
    #[default]
    #[serde(other)]
    Unscanned,
}

// ---------------------------------------------------------------------------
// Per-version security scan summary (modelled after ClawHub scan data)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanVerdict {
    #[default]
    Pending,
    Benign,
    Suspicious,
    Malicious,
}

impl ScanVerdict {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Benign => "Benign",
            Self::Suspicious => "Suspicious",
            Self::Malicious => "Malicious",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VtScanResult {
    pub verdict: ScanVerdict,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmScanDimension {
    pub name: String,
    pub label: String,
    pub rating: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmScanResult {
    pub verdict: ScanVerdict,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<LlmScanDimension>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticScanFinding {
    pub code: String,
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticScanResult {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<StaticScanFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reason_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionScanSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virustotal: Option<VtScanResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_analysis: Option<LlmScanResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_scan: Option<StaticScanResult>,
}

impl VersionScanSummary {
    /// Overall verdict across all scanners (worst wins).
    pub fn overall_verdict(&self) -> ScanVerdict {
        let verdicts = [
            self.virustotal.as_ref().map(|v| v.verdict),
            self.llm_analysis.as_ref().map(|v| v.verdict),
        ];
        if verdicts.contains(&Some(ScanVerdict::Malicious)) {
            return ScanVerdict::Malicious;
        }
        if verdicts.contains(&Some(ScanVerdict::Suspicious)) {
            return ScanVerdict::Suspicious;
        }
        if verdicts.iter().all(|v| *v == Some(ScanVerdict::Benign)) {
            return ScanVerdict::Benign;
        }
        ScanVerdict::Pending
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexJobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitIndexRequest {
    pub git_url: String,
    #[serde(default = "default_git_ref")]
    pub git_ref: String,
    #[serde(default = "default_git_subdir")]
    pub git_subdir: String,
    pub repo_slug: Option<String>,
    #[serde(default)]
    pub force: bool,
}

fn default_git_ref() -> String {
    "main".to_string()
}

fn default_git_subdir() -> String {
    ".".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitIndexResponse {
    pub ok: bool,
    pub job_id: Uuid,
    /// True when an existing completed scan with the same commit was found.
    #[serde(default)]
    pub skipped: bool,
    /// The job_id of the existing scan, when skipped.
    pub existing_job_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexJobDto {
    pub id: Uuid,
    pub status: IndexJobStatus,
    pub job_type: String,
    pub git_url: String,
    pub git_ref: String,
    pub git_subdir: String,
    pub repo_slug: Option<String>,
    pub result_data: Value,
    pub error_message: Option<String>,
    pub progress_pct: i32,
    pub progress_message: String,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexJobListResponse {
    pub jobs: Vec<IndexJobDto>,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowseHistoryItem {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub resource_slug: String,
    pub resource_title: String,
    pub owner_handle: Option<String>,
    pub viewed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowseHistoryResponse {
    pub items: Vec<BrowseHistoryItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordViewRequest {
    pub resource_type: String,
    pub resource_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordInstallRequest {
    #[serde(default)]
    pub client_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityScanDto {
    pub id: Uuid,
    pub scan_module: String,
    pub result: String,
    pub severity: Option<String>,
    pub details: Value,
    pub scanned_by: Option<UserSummary>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityScanListResponse {
    pub scans: Vec<SecurityScanDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSecurityStatusRequest {
    pub security_status: SecurityStatus,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteAdminDto {
    pub id: Uuid,
    pub user: UserSummary,
    pub granted_by: Option<UserSummary>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteAdminListResponse {
    pub admins: Vec<SiteAdminDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddSiteAdminRequest {
    pub user_handle: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminActionResponse {
    pub ok: bool,
    pub message: String,
}

// -- Admin Index Jobs --

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminIndexJobDto {
    pub id: Uuid,
    pub status: IndexJobStatus,
    pub job_type: String,
    pub git_url: String,
    pub git_ref: String,
    pub git_subdir: String,
    pub repo_slug: Option<String>,
    pub result_data: Value,
    pub error_message: Option<String>,
    pub progress_pct: i32,
    pub progress_message: String,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub requested_by: UserSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminIndexJobListResponse {
    pub jobs: Vec<AdminIndexJobDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelIndexJobResponse {
    pub ok: bool,
    pub job_id: Uuid,
    pub status: IndexJobStatus,
}

// -- Index Rules --

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexRuleDto {
    pub id: Uuid,
    pub repo_url: String,
    pub path_regex: String,
    pub strategy: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexRuleListResponse {
    pub rules: Vec<IndexRuleDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateIndexRuleRequest {
    pub repo_url: String,
    pub path_regex: String,
    pub strategy: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateIndexRuleRequest {
    pub repo_url: Option<String>,
    pub path_regex: Option<String>,
    pub strategy: Option<String>,
    pub description: Option<String>,
}

// -- Official Selectors --

/// Response from `GET /api/v1/presets`.
///
/// Each entry is a `serde_json::Value` matching the `SelectorDefinition`
/// shape plus optional `tags`.  The client-local library converts these into
/// typed `OfficialSelectorEntry` values after fetching.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfficialSelectorsResponse {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    pub selectors: Vec<Value>,
}

// -- Documentation --

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocPageResponse {
    pub title: String,
    pub description: String,
    pub content_html: String,
    pub toc: Vec<DocTocItem>,
    pub sidebar: Vec<DocSidebarGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocTocItem {
    pub id: String,
    pub text: String,
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSidebarGroup {
    pub title: String,
    pub items: Vec<DocSidebarLink>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSidebarLink {
    pub text: String,
    pub link: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Skill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Admin,
    Moderator,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModerationStatus {
    Active,
    Hidden,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogStats {
    pub downloads: i64,
    pub stars: i64,
    pub versions: i64,
    pub comments: i64,
    #[serde(default)]
    pub installs: i64,
    #[serde(default)]
    pub unique_users: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillBadges {
    pub highlighted: bool,
    pub official: bool,
    pub deprecated: bool,
    pub suspicious: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserSummary {
    pub id: Uuid,
    pub handle: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: UserRole,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionSummary {
    pub id: Uuid,
    pub version: String,
    pub changelog: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_summary: Option<VersionScanSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceFileSummary {
    pub path: String,
    pub size: i32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceFile {
    pub path: String,
    pub content: String,
    pub size: i32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionDetail {
    pub id: Uuid,
    pub version: String,
    pub changelog: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub files: Vec<ResourceFileSummary>,
    pub markdown_html: String,
    pub parsed_metadata: Value,
    pub bundle_metadata: Option<BundleMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_summary: Option<VersionScanSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillListItem {
    pub id: Uuid,
    pub slug: String,
    pub path: String,
    pub display_name: String,
    pub summary: Option<String>,
    pub repo_id: String,
    #[serde(default)]
    pub repo_url: String,
    pub owner: UserSummary,
    pub tags: IndexMap<String, String>,
    pub stats: CatalogStats,
    pub badges: SkillBadges,
    pub moderation_status: ModerationStatus,
    #[serde(default)]
    pub security_status: SecurityStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub latest_version: Option<VersionSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommentDto {
    pub id: Uuid,
    pub user: UserSummary,
    pub body: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub can_delete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillDetailResponse {
    pub skill: SkillListItem,
    pub latest_version: Option<VersionDetail>,
    pub versions: Vec<VersionSummary>,
    pub comments: Vec<CommentDto>,
    pub starred: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub kind: ResourceKind,
    pub slug: String,
    pub display_name: String,
    pub summary: Option<String>,
    pub score: f32,
    pub updated_at: DateTime<Utc>,
    pub latest_version: Option<String>,
    pub owner_handle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PagedResponse<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishBundleFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexRequest {
    pub slug: String,
    pub display_name: String,
    pub version: String,
    pub changelog: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub files: Vec<PublishBundleFile>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishResponse {
    pub ok: bool,
    pub resource_id: Uuid,
    pub version_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateCommentRequest {
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToggleStarResponse {
    pub ok: bool,
    pub stars: i64,
    pub starred: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveCustomSelectorsRequest {
    pub selectors: Vec<serde_json::Value>,
    #[serde(default)]
    pub version: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomSelectorsResponse {
    pub ok: bool,
    pub selectors: Vec<serde_json::Value>,
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

/// D5: response from POST /me/selectors/custom/validate.
///
/// Reports per-entry validation issues without persisting the payload, so the
/// custom-selector editor UI can surface inline errors before the user clicks
/// save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectorValidationIssue {
    /// Index in the submitted `selectors` array.
    pub index: usize,
    /// Field that failed (e.g. "name", "kind", "pattern") — empty if the
    /// whole entry is invalid.
    #[serde(default)]
    pub field: String,
    /// Human-readable error message.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidateCustomSelectorsResponse {
    /// `true` when `issues` is empty.
    pub ok: bool,
    pub issues: Vec<SelectorValidationIssue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StarredIdsResponse {
    pub skill_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhoAmIResponse {
    pub user: Option<UserSummary>,
    pub token_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserProfileStats {
    pub published_skills: i64,
    pub starred_skills: i64,
    pub starred_flocks: i64,
    pub history: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserProfileResponse {
    pub user: UserSummary,
    pub bio: Option<String>,
    pub joined_at: DateTime<Utc>,
    pub github_login: Option<String>,
    pub is_self: bool,
    pub stats: UserProfileStats,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserListItem {
    pub user: UserSummary,
    pub skill_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserListResponse {
    pub items: Vec<UserListItem>,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModerationUpdateRequest {
    pub status: ModerationStatus,
    pub highlighted: Option<bool>,
    pub official: Option<bool>,
    pub deprecated: Option<bool>,
    pub suspicious: Option<bool>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: Uuid,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<Uuid>,
    pub actor: Option<UserSummary>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileContentResponse {
    pub path: String,
    pub content: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedVersion {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolveResponse {
    pub slug: String,
    #[serde(rename = "match")]
    pub matched: Option<ResolvedVersion>,
    pub latest_version: Option<ResolvedVersion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogCounts {
    pub users: i64,
    pub repos: i64,
    pub flocks: i64,
    pub skills: i64,
    pub versions: i64,
    pub comments: i64,
    pub reports: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagementSummaryResponse {
    pub counts: CatalogCounts,
    pub audit_logs: Vec<AuditLogEntry>,
    #[serde(default)]
    pub ai_usage: Vec<AiUsageSummaryItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiUsageSummaryItem {
    pub task_type: String,
    pub model: String,
    pub call_count: i64,
    pub total_prompt_tokens: i64,
    pub total_completion_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetUserRoleRequest {
    pub role: UserRole,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleUpdateResponse {
    pub ok: bool,
    pub user: UserSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BanUserRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BanUserResponse {
    pub ok: bool,
    pub user: UserSummary,
    pub revoked_tokens: i64,
    pub deleted_skills: i64,
}

// --- Reports ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportReason {
    Spam,
    Abuse,
    Inappropriate,
    Copyright,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Pending,
    Reviewed,
    Resolved,
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportTargetType {
    Skill,
    Flock,
    Comment,
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateReportRequest {
    pub target_type: ReportTargetType,
    pub target_id: Uuid,
    pub reason: ReportReason,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportDto {
    pub id: Uuid,
    pub reporter: UserSummary,
    pub target_type: ReportTargetType,
    pub target_id: Uuid,
    pub reason: ReportReason,
    pub description: String,
    pub status: ReportStatus,
    pub reviewed_by: Option<UserSummary>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportListResponse {
    pub reports: Vec<ReportDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewReportRequest {
    pub status: ReportStatus,
    pub notes: Option<String>,
}

// --- Flock Blocks (Blacklist) ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockedFlockDto {
    pub flock_id: Uuid,
    pub repo_slug: String,
    pub flock_slug: String,
    pub flock_name: String,
    pub blocked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlockBlockListResponse {
    pub blocked_flocks: Vec<BlockedFlockDto>,
}

// --- Flock Ratings ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateFlockRequest {
    pub score: i16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlockRatingStats {
    pub count: i64,
    pub average: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateFlockResponse {
    pub ok: bool,
    pub score: i16,
    pub stats: FlockRatingStats,
}
