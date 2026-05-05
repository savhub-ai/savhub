/// Internationalization — English / Chinese.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    English,
    Chinese,
}

impl Language {
    pub fn code(&self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Chinese => "zh",
        }
    }

    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Chinese => "中文",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code {
            "zh" => Language::Chinese,
            _ => Language::English,
        }
    }
}

#[allow(dead_code)]
pub struct Texts {
    pub lang: Language,

    // App
    pub app_name: &'static str,
    pub app_window_title: &'static str,

    // Common
    pub loading: &'static str,
    pub fetch: &'static str,
    pub fetched: &'static str,
    pub fetching: &'static str,
    pub no_version: &'static str,

    // Sidebar
    pub nav_dashboard: &'static str,
    pub nav_explore: &'static str,
    pub nav_selectors: &'static str,
    pub nav_docs: &'static str,
    pub nav_settings: &'static str,

    // Dashboard
    pub dashboard_title: &'static str,
    pub registry_status: &'static str,
    pub dashboard_account: &'static str,
    pub fetched_skills: &'static str,
    pub dashboard_recent_projects: &'static str,
    pub dashboard_no_recent_projects: &'static str,
    pub dashboard_recent_added: &'static str,
    pub dashboard_recent_updated: &'static str,
    pub detected_ai_agents: &'static str,
    pub no_ai_agents_detected: &'static str,
    pub checking: &'static str,
    pub offline: &'static str,
    pub connection_details: &'static str,
    pub anonymous: &'static str,
    pub not_logged_in: &'static str,

    // Explore
    pub explore_title: &'static str,
    pub search_placeholder: &'static str,
    pub search: &'static str,
    pub filter_all: &'static str,
    pub no_skills_found: &'static str,
    pub grouped_label: &'static str,
    pub no_flocks_found: &'static str,
    pub flock_skills_count: &'static str,
    pub flock_detail_title: &'static str,
    pub flock_fetch_all: &'static str,
    pub flock_prune_all: &'static str,
    pub flock_back: &'static str,
    pub by: &'static str,
    pub show_as_cards: &'static str,
    pub show_as_list: &'static str,

    // Installed
    pub update_all: &'static str,
    pub no_skills_fetched: &'static str,
    pub no_skills_fetched_hint: &'static str,
    pub prune: &'static str,
    pub fetched_at_prefix: &'static str,

    // Settings
    pub settings_title: &'static str,
    pub authentication: &'static str,
    pub authenticated_token_set: &'static str,
    pub logout: &'static str,
    pub logged_out: &'static str,
    pub not_logged_in_hint: &'static str,
    pub logging_in: &'static str,
    pub login_with_github: &'static str,
    pub registry_url: &'static str,
    pub bearer_token: &'static str,
    pub token_hint: &'static str,
    pub browse: &'static str,
    pub save_settings: &'static str,
    pub settings_saved: &'static str,
    pub language_label: &'static str,
    pub opening_browser: &'static str,
    pub login_succeeded_no_user: &'static str,
    pub timed_out_login: &'static str,

    // Common (shared across pages)
    pub clear: &'static str,
    pub close: &'static str,
    pub not_found: &'static str,

    // Settings tabs
    pub settings_general: &'static str,
    pub settings_account: &'static str,
    pub settings_about: &'static str,

    // About
    pub about_version: &'static str,
    pub about_check_update: &'static str,
    pub about_checking: &'static str,
    pub about_up_to_date: &'static str,
    pub about_copyright: &'static str,
    pub about_license: &'static str,
    pub about_github: &'static str,
    pub about_open: &'static str,

    // Update
    pub update_download: &'static str,
    pub update_downloading: &'static str,
    pub update_ready: &'static str,
    pub update_restart: &'static str,
    pub update_dismiss: &'static str,

    // Registry API compatibility
    pub compat_incompatible: &'static str,
    pub compat_update_now: &'static str,

    // Security
    pub security_unscanned: &'static str,
    pub security_checked: &'static str,
    pub security_scanning: &'static str,
    pub security_verified: &'static str,
    pub security_suspicious: &'static str,
    pub security_malicious: &'static str,
    pub security_scan: &'static str,
    pub scan_virustotal: &'static str,
    pub scan_llm_analysis: &'static str,
    pub scan_static: &'static str,
    pub scan_benign: &'static str,
    pub scan_suspicious: &'static str,
    pub scan_malicious: &'static str,
    pub scan_pending: &'static str,

    // Detail
    pub back_to_explore: &'static str,
    pub latest_version: &'static str,
    pub statistics: &'static str,
    pub downloads: &'static str,
    pub stars: &'static str,
    pub versions: &'static str,
    pub comments: &'static str,
    pub installs: &'static str,
    pub unique_users: &'static str,
    pub changelog: &'static str,
    pub version_history: &'static str,
    pub files: &'static str,

    // Sidebar - new
    pub nav_profiles: &'static str,

    // Selectors page
    pub selectors_title: &'static str,
    pub selectors_description: &'static str,
    pub selectors_rule_title: &'static str,
    pub selectors_rule_hint: &'static str,
    pub selectors_empty_title: &'static str,
    pub selectors_empty_hint: &'static str,
    pub selectors_create: &'static str,
    pub selectors_use_template: &'static str,
    pub selectors_edit: &'static str,
    pub selectors_delete: &'static str,
    pub selectors_enable: &'static str,
    pub selectors_disable: &'static str,
    pub selectors_save: &'static str,
    pub selectors_cancel: &'static str,
    pub selectors_new_title: &'static str,
    pub selectors_edit_title: &'static str,
    pub selectors_name_label: &'static str,
    pub selectors_desc_label: &'static str,
    pub selectors_scope_label: &'static str,
    pub selectors_rules_label: &'static str,
    pub selectors_add_rule: &'static str,
    pub selectors_match_all: &'static str,
    pub selectors_match_any: &'static str,
    pub selectors_match_custom: &'static str,
    pub selectors_expr_label: &'static str,
    pub selectors_expr_hint: &'static str,
    pub selectors_file_exists: &'static str,
    pub selectors_folder_exists: &'static str,
    pub selectors_no_selectors: &'static str,
    pub selectors_add_skills_label: &'static str,
    pub selectors_add_tag: &'static str,
    pub selectors_glob_match: &'static str,
    pub selectors_file_contains: &'static str,
    pub selectors_file_regex: &'static str,
    pub selectors_env_var_set: &'static str,
    pub selectors_command_exits: &'static str,
    pub selectors_search_skills: &'static str,
    pub selectors_add_flocks_label: &'static str,
    pub selectors_search_flocks: &'static str,
    pub selectors_add_repos_label: &'static str,
    pub selectors_repos_placeholder: &'static str,
    pub selectors_repos_hint: &'static str,
    pub selectors_priority: &'static str,
    pub selectors_official_title: &'static str,
    pub selectors_custom_title: &'static str,
    pub selectors_enable_all: &'static str,
    pub selectors_disable_all: &'static str,
    pub selectors_clone_template: &'static str,
    pub selectors_syncing: &'static str,
    pub selectors_sync_now: &'static str,
    pub selectors_last_synced: &'static str,
    pub selectors_official_badge: &'static str,

    // Projects page
    pub projects_title: &'static str,
    pub projects_add: &'static str,
    pub projects_remove: &'static str,
    pub projects_no_projects: &'static str,
    pub projects_path_placeholder: &'static str,
    pub project_no_profile: &'static str,
    pub project_enabled_skills: &'static str,
    pub project_inject_skill: &'static str,
    pub project_inject_placeholder: &'static str,
    pub project_select_hint: &'static str,
    pub project_matched_selectors: &'static str,
    pub project_no_selectors: &'static str,
    pub project_no_enabled_skills: &'static str,
    pub project_skill_name: &'static str,
    pub project_skill_reason: &'static str,
    pub project_skill_action: &'static str,
    pub project_reason_selectors: &'static str,
    pub project_reason_flocks: &'static str,
    pub project_reason_manual: &'static str,
    pub project_local_skills_title: &'static str,
    pub project_local_skills_empty: &'static str,
    pub project_local_flocks_title: &'static str,
    pub project_local_flocks_empty: &'static str,
    pub project_inject_flock: &'static str,
    pub project_use_repo_skill: &'static str,
    pub project_keep_existing_skill: &'static str,
    pub project_conflict_detected: &'static str,

    // Rescan / Apply
    pub project_rescan: &'static str,
    pub project_rescan_title: &'static str,
    pub project_rescan_no_match: &'static str,
    pub project_rescan_matched: &'static str,
    pub project_rescan_skills: &'static str,
    pub project_rescan_flocks: &'static str,
    pub project_rescan_apply: &'static str,
    pub project_rescan_applying: &'static str,
    pub project_rescan_done: &'static str,
    pub project_rescan_close: &'static str,

    // Settings: AI Agents
    pub settings_agents: &'static str,
    pub settings_agents_auto: &'static str,
    pub settings_agents_manual: &'static str,
    pub settings_agents_hint: &'static str,
}

/// Format helpers for dynamic strings.
impl Texts {
    pub fn fmt_updated_skills(&self, updated: usize, total: usize) -> String {
        match self.lang {
            Language::English => format!("Updated {updated}/{total} skills."),
            Language::Chinese => format!("已更新 {updated}/{total} 个技能。"),
        }
    }

    pub fn fmt_files_count(&self, count: usize) -> String {
        match self.lang {
            Language::English => format!("Files ({count})"),
            Language::Chinese => format!("文件（{count}）"),
        }
    }

    pub fn fmt_logged_in_via_github(&self, handle: &str) -> String {
        match self.lang {
            Language::English => format!("GitHub: @{handle}"),
            Language::Chinese => format!("GitHub：@{handle}"),
        }
    }

    pub fn fmt_login_failed(&self, err: &str) -> String {
        match self.lang {
            Language::English => format!("Login failed: {err}"),
            Language::Chinese => format!("登录失败：{err}"),
        }
    }

    pub fn fmt_login_verify_failed(&self, err: &str) -> String {
        match self.lang {
            Language::English => format!("Login token received but verification failed: {err}"),
            Language::Chinese => format!("已收到登录令牌但验证失败：{err}"),
        }
    }

    pub fn fmt_update_available(&self, version: &str) -> String {
        match self.lang {
            Language::English => format!("A new version v{version} is available."),
            Language::Chinese => {
                format!("新版本 v{version} 已发布。")
            }
        }
    }

    pub fn fmt_update_failed(&self, err: &str) -> String {
        match self.lang {
            Language::English => format!("Update failed: {err}"),
            Language::Chinese => format!("更新失败：{err}"),
        }
    }

    pub fn fmt_compat_detail(&self, client_ver: u32, registry_ver: u32) -> String {
        match self.lang {
            Language::English => {
                format!("Client API: v{client_ver}, Registry API: v{registry_ver}")
            }
            Language::Chinese => format!("客户端 API：v{client_ver}，注册表 API：v{registry_ver}"),
        }
    }
}

static EN: Texts = Texts {
    lang: Language::English,

    app_name: "Savhub",
    app_window_title: "Savhub Desktop",

    loading: "Loading...",
    fetch: "Fetch",
    fetched: "Fetched",
    fetching: "Fetching...",
    no_version: "No version available",

    nav_dashboard: "Dashboard",
    nav_explore: "Skills",
    nav_selectors: "Selectors",
    nav_docs: "Docs",
    nav_settings: "Settings",
    dashboard_title: "Dashboard",
    registry_status: "Registry Status",
    dashboard_account: "Account",
    fetched_skills: "Fetched Skills",
    dashboard_recent_projects: "Recent Projects",
    dashboard_no_recent_projects: "No recent projects yet.",
    dashboard_recent_added: "Added",
    dashboard_recent_updated: "Updated",
    detected_ai_agents: "Detected AI Agents",
    no_ai_agents_detected: "No AI agents detected on this PC.",
    checking: "checking...",
    offline: "offline",
    connection_details: "Connection Details",
    anonymous: "anonymous",
    not_logged_in: "not logged in",

    explore_title: "Skills",
    search_placeholder: "Search skills...",
    search: "Search",
    filter_all: "All",
    no_skills_found: "No skills found.",
    grouped_label: "Grouped",
    no_flocks_found: "No flocks found.",
    flock_skills_count: "skills",
    flock_detail_title: "Flock Details",
    flock_fetch_all: "Fetch All",
    flock_prune_all: "Prune All",
    flock_back: "Back",
    by: "by",
    show_as_cards: "Show cards",
    show_as_list: "Show list",

    update_all: "Update All",
    no_skills_fetched: "No skills fetched in this project.",
    no_skills_fetched_hint: "Go to Skills to find and fetch skills.",
    prune: "Prune",
    fetched_at_prefix: "Fetched",

    settings_title: "Settings",
    authentication: "Authentication",
    authenticated_token_set: "Authenticated (token set)",
    logout: "Logout",
    logged_out: "Logged out.",
    not_logged_in_hint: "Not logged in. Login via GitHub to manage skills.",
    logging_in: "Logging in...",
    login_with_github: "Login with GitHub",
    registry_url: "Registry URL",
    bearer_token: "Bearer Token",
    token_hint: "Set automatically after GitHub login. You can also paste a token manually.",
    browse: "Browse",
    save_settings: "Save Settings",
    settings_saved: "Settings saved.",
    language_label: "Language",
    opening_browser: "Opening browser for GitHub login...",
    login_succeeded_no_user: "Login succeeded but no user found.",
    timed_out_login: "Timed out waiting for GitHub login.",

    clear: "Clear",
    close: "Close",
    not_found: "Not found",

    settings_general: "General",
    settings_account: "Account",
    settings_about: "About",

    about_version: "Version",
    about_check_update: "Check for Updates",
    about_checking: "Checking...",
    about_up_to_date: "You are up to date.",
    about_copyright: "\u{00A9} 2025 Savhub. All rights reserved.",
    about_license: "License",
    about_github: "GitHub Repository",
    about_open: "Open",

    update_download: "Download & Install",
    update_downloading: "Downloading update...",
    update_ready: "Update downloaded. Restart to apply.",
    update_restart: "Restart Now",
    update_dismiss: "Later",

    compat_incompatible: "Registry API version is incompatible with this client. Data may not work correctly. Please update immediately.",
    compat_update_now: "Check for Updates",

    security_unscanned: "Unscanned",
    security_checked: "Validated",
    security_scanning: "Scanning",
    security_verified: "Verified",
    security_suspicious: "Suspicious",
    security_malicious: "Malicious",
    security_scan: "Security Scan",
    scan_virustotal: "VirusTotal",
    scan_llm_analysis: "LLM Analysis",
    scan_static: "Static Scan",
    scan_benign: "Benign",
    scan_suspicious: "Suspicious",
    scan_malicious: "Malicious",
    scan_pending: "Pending",

    back_to_explore: "\u{2190} Back to Skills",
    latest_version: "Latest Version",
    statistics: "Statistics",
    downloads: "Downloads",
    stars: "Stars",
    versions: "Versions",
    comments: "Comments",
    installs: "Installs",
    unique_users: "Users",
    changelog: "Changelog",
    version_history: "Version History",
    files: "Files",

    nav_profiles: "Projects",

    selectors_title: "Selectors",
    selectors_description: "Selectors inspect a folder, check for required files, and apply skill changes. Rules can be combined with AND, OR, NOT and parentheses for complex matching logic.",
    selectors_rule_title: "Rules",
    selectors_rule_hint: "File and folder existence checks scoped to a directory.",
    selectors_empty_title: "No Selectors",
    selectors_empty_hint: "Create a selector to automatically detect project types and apply skills.",
    selectors_create: "Create Selector",
    selectors_use_template: "Use as Template",
    selectors_edit: "Edit",
    selectors_delete: "Delete",
    selectors_enable: "Enable",
    selectors_disable: "Disable",
    selectors_save: "Save",
    selectors_cancel: "Cancel",
    selectors_new_title: "New Selector",
    selectors_edit_title: "Edit Selector",
    selectors_name_label: "Name",
    selectors_desc_label: "Description",
    selectors_scope_label: "Folder Scope",
    selectors_rules_label: "Rules",
    selectors_add_rule: "Add Rule",
    selectors_match_all: "All Match",
    selectors_match_any: "Any Match",
    selectors_match_custom: "Custom",
    selectors_expr_label: "Expression",
    selectors_expr_hint: "Rule numbers with && (AND), || (OR), ! (NOT), (). Example: (1 && 2) || !3",
    selectors_file_exists: "File Exists",
    selectors_folder_exists: "Folder Exists",
    selectors_no_selectors: "No selectors configured yet.",
    selectors_add_skills_label: "Skills",
    selectors_add_tag: "Add",
    selectors_glob_match: "Glob Match",
    selectors_file_contains: "File Contains",
    selectors_file_regex: "File Regex",
    selectors_env_var_set: "Env Var Set",
    selectors_command_exits: "Command Exits",
    selectors_search_skills: "Search skills...",
    selectors_add_flocks_label: "Flocks (Grouped Skills)",
    selectors_search_flocks: "Search flocks...",
    selectors_add_repos_label: "Repos",
    selectors_repos_placeholder: "https://github.com/owner/repo or git@github.com:owner/repo",
    selectors_repos_hint: "When matched, all flocks and skills from these repos will be fetched. URLs are normalized automatically.",
    selectors_priority: "Priority",
    selectors_official_title: "Official",
    selectors_custom_title: "Custom",
    selectors_enable_all: "Enable All",
    selectors_disable_all: "Disable All",
    selectors_clone_template: "Clone",
    selectors_syncing: "Syncing...",
    selectors_sync_now: "Sync Now",
    selectors_last_synced: "Synced",
    selectors_official_badge: "Official",

    projects_title: "Projects",
    projects_add: "Add Project",
    projects_remove: "Remove",
    projects_no_projects: "No projects added yet. Add a project directory to get started.",
    projects_path_placeholder: "Project directory path...",
    project_no_profile: "No skills enabled",
    project_enabled_skills: "Enabled Skills",
    project_inject_skill: "Add Skill",
    project_inject_placeholder: "skill-slug",
    project_select_hint: "Select a project from the list to view details.",
    project_matched_selectors: "Matched Selectors",
    project_no_selectors: "No selectors matched this project yet.",
    project_no_enabled_skills: "No enabled skills in this project.",
    project_skill_name: "Skill",
    project_skill_reason: "Why Enabled",
    project_skill_action: "Action",
    project_reason_selectors: "Selectors",
    project_reason_flocks: "Flocks",
    project_reason_manual: "Manual",
    project_local_skills_title: "Add Skills",
    project_local_skills_empty: "No fetched skills found in ~/.savhub/fetched.json.",
    project_local_flocks_title: "Add Flocks",
    project_local_flocks_empty: "No fetched flocks found. Run `savhub fetch` for a flock skill first.",
    project_inject_flock: "Add Flock",
    project_use_repo_skill: "Use Repo Skill",
    project_keep_existing_skill: "Keep Existing",
    project_conflict_detected: "A skill with the same name already exists in this project.",

    project_rescan: "Rescan & Apply",
    project_rescan_title: "Selector Scan Results",
    project_rescan_no_match: "No selectors matched this project. Previously applied skills will be removed.",
    project_rescan_matched: "Matched Selectors",
    project_rescan_skills: "Skills to fetch",
    project_rescan_flocks: "Flocks",
    project_rescan_apply: "Apply",
    project_rescan_applying: "Applying...",
    project_rescan_done: "Applied successfully.",
    project_rescan_close: "Close",

    settings_agents: "AI Agents",
    settings_agents_auto: "Auto-detect",
    settings_agents_manual: "Manual",
    settings_agents_hint: "Choose which AI agents to sync skills to. Auto-detect checks installed agents on your machine.",
};

static ZH: Texts = Texts {
    lang: Language::Chinese,

    app_name: "智汇坊",
    app_window_title: "智汇坊 Desktop",

    loading: "加载中...",
    fetch: "获取",
    fetched: "已获取",
    fetching: "获取中...",
    no_version: "无可用版本",

    nav_dashboard: "仪表盘",
    nav_explore: "技能",
    nav_selectors: "选择器",
    nav_docs: "文档",
    nav_settings: "设置",

    dashboard_title: "仪表盘",
    registry_status: "注册表状态",
    dashboard_account: "账户",
    fetched_skills: "已获取技能",
    dashboard_recent_projects: "最近项目",
    dashboard_no_recent_projects: "暂无最近项目。",
    dashboard_recent_added: "新增",
    dashboard_recent_updated: "更新",
    detected_ai_agents: "已检测到的 AI 客户端",
    no_ai_agents_detected: "未在此电脑上检测到 AI 客户端。",
    checking: "检查中...",
    offline: "离线",
    connection_details: "连接详情",
    anonymous: "匿名",
    not_logged_in: "未登录",

    explore_title: "技能",
    search_placeholder: "搜索技能...",
    search: "搜索",
    filter_all: "全部",
    no_skills_found: "未找到技能。",
    grouped_label: "按集合",
    no_flocks_found: "未找到技能集。",
    flock_skills_count: "个技能",
    flock_detail_title: "技能集详情",
    flock_fetch_all: "全部获取",
    flock_prune_all: "全部移除",
    flock_back: "返回",
    by: "作者",
    show_as_cards: "显示为卡片",
    show_as_list: "显示为列表",

    update_all: "全部更新",
    no_skills_fetched: "当前项目没有已获取的技能。",
    no_skills_fetched_hint: "前往「技能」页面查找并获取技能。",
    prune: "移除",
    fetched_at_prefix: "获取于",

    settings_title: "设置",
    authentication: "身份验证",
    authenticated_token_set: "已认证（令牌已设置）",
    logout: "登出",
    logged_out: "已登出。",
    not_logged_in_hint: "未登录。通过 GitHub 登录以管理技能。",
    logging_in: "登录中...",
    login_with_github: "通过 GitHub 登录",
    registry_url: "注册表地址",
    bearer_token: "访问令牌",
    token_hint: "GitHub 登录后自动设置，也可手动粘贴令牌。",
    browse: "浏览",
    save_settings: "保存设置",
    settings_saved: "设置已保存。",
    language_label: "语言",
    opening_browser: "正在打开浏览器进行 GitHub 登录...",
    login_succeeded_no_user: "登录成功但未找到用户。",
    timed_out_login: "GitHub 登录超时。",

    clear: "清除",
    close: "关闭",
    not_found: "未找到",

    settings_general: "通用",
    settings_account: "账户",
    settings_about: "关于",

    about_version: "版本",
    about_check_update: "检查更新",
    about_checking: "检查中...",
    about_up_to_date: "已是最新版本。",
    about_copyright: "\u{00A9} 2025 智汇坊。保留所有权利。",
    about_license: "许可证",
    about_github: "GitHub 仓库",
    about_open: "打开",

    update_download: "下载并安装",
    update_downloading: "正在下载更新...",
    update_ready: "更新已下载，重启以应用。",
    update_restart: "立即重启",
    update_dismiss: "稍后",

    compat_incompatible: "注册表 API 版本与此客户端不兼容，数据可能无法正常使用。请立即更新客户端。",
    compat_update_now: "检查更新",

    security_unscanned: "未扫描",
    security_checked: "已检查",
    security_scanning: "扫描中",
    security_verified: "已验证",
    security_suspicious: "可疑",
    security_malicious: "恶意",
    security_scan: "安全扫描",
    scan_virustotal: "VirusTotal",
    scan_llm_analysis: "LLM 分析",
    scan_static: "静态扫描",
    scan_benign: "安全",
    scan_suspicious: "可疑",
    scan_malicious: "恶意",
    scan_pending: "待扫描",

    back_to_explore: "\u{2190} 返回技能",
    latest_version: "最新版本",
    statistics: "统计",
    downloads: "下载量",
    stars: "收藏数",
    versions: "版本数",
    comments: "评论数",
    installs: "安装量",
    unique_users: "使用者",
    changelog: "更新日志",
    version_history: "版本历史",
    files: "文件",

    nav_profiles: "项目",

    selectors_title: "选择器",
    selectors_description: "选择器检查文件夹中的文件，并应用技能操作。规则可用 AND、OR、NOT 和括号组合，实现复杂的匹配逻辑。",
    selectors_rule_title: "规则",
    selectors_rule_hint: "限定到某个目录的文件和文件夹存在检查。",
    selectors_empty_title: "暂无选择器",
    selectors_empty_hint: "创建选择器以自动识别项目类型并应用技能。",
    selectors_create: "创建选择器",
    selectors_use_template: "用作模板",
    selectors_edit: "编辑",
    selectors_delete: "删除",
    selectors_enable: "启用",
    selectors_disable: "禁用",
    selectors_save: "保存",
    selectors_cancel: "取消",
    selectors_new_title: "新建选择器",
    selectors_edit_title: "编辑选择器",
    selectors_name_label: "名称",
    selectors_desc_label: "描述",
    selectors_scope_label: "文件夹范围",
    selectors_rules_label: "规则",
    selectors_add_rule: "添加规则",
    selectors_match_all: "全部匹配",
    selectors_match_any: "任一匹配",
    selectors_match_custom: "自定义",
    selectors_expr_label: "表达式",
    selectors_expr_hint: "用规则编号配合 && (AND)、|| (OR)、! (NOT) 和括号。示例：(1 && 2) || !3",
    selectors_file_exists: "文件存在",
    selectors_folder_exists: "文件夹存在",
    selectors_no_selectors: "还没有配置任何选择器。",
    selectors_add_skills_label: "技能",
    selectors_add_tag: "添加",
    selectors_glob_match: "Glob 匹配",
    selectors_file_contains: "文件包含",
    selectors_file_regex: "文件正则",
    selectors_env_var_set: "环境变量",
    selectors_command_exits: "命令执行",
    selectors_search_skills: "搜索技能...",
    selectors_add_flocks_label: "技能集（分组技能）",
    selectors_search_flocks: "搜索技能集...",
    selectors_add_repos_label: "仓库",
    selectors_repos_placeholder: "https://github.com/owner/repo 或 git@github.com:owner/repo",
    selectors_repos_hint: "匹配时将获取这些仓库中的所有技能集和技能。URL 会自动标准化。",
    selectors_priority: "优先级",
    selectors_official_title: "官方",
    selectors_custom_title: "自定义",
    selectors_enable_all: "全部启用",
    selectors_disable_all: "全部禁用",
    selectors_clone_template: "克隆",
    selectors_syncing: "同步中...",
    selectors_sync_now: "立即同步",
    selectors_last_synced: "已同步",
    selectors_official_badge: "官方",

    projects_title: "项目",
    projects_add: "添加项目",
    projects_remove: "移除",
    projects_no_projects: "暂无项目。添加项目目录以开始。",
    projects_path_placeholder: "项目目录路径...",
    project_no_profile: "未启用技能",
    project_enabled_skills: "已启用技能",
    project_inject_skill: "添加技能",
    project_inject_placeholder: "skill-slug",
    project_select_hint: "从列表中选择项目以查看详情。",
    project_matched_selectors: "已匹配的选择器",
    project_no_selectors: "还没有选择器匹配到这个项目。",
    project_no_enabled_skills: "这个项目还没有启用的技能。",
    project_skill_name: "技能",
    project_skill_reason: "启用原因",
    project_skill_action: "操作",
    project_reason_selectors: "选择器",
    project_reason_flocks: "技能集",
    project_reason_manual: "手动",
    project_local_skills_title: "添加技能",
    project_local_skills_empty: "在 ~/.savhub/fetched.json 中未找到已获取的技能。",
    project_local_flocks_title: "添加技能集",
    project_local_flocks_empty: "尚未获取任何技能集。请先对其中的技能运行 `savhub fetch`。",
    project_inject_flock: "添加技能集",
    project_use_repo_skill: "使用仓库技能",
    project_keep_existing_skill: "保留现有",
    project_conflict_detected: "这个项目中已存在同名技能。",

    project_rescan: "重新扫描并应用",
    project_rescan_title: "选择器扫描结果",
    project_rescan_no_match: "没有选择器匹配此项目。之前应用的技能将被移除。",
    project_rescan_matched: "匹配的选择器",
    project_rescan_skills: "待获取技能",
    project_rescan_flocks: "技能集",
    project_rescan_apply: "应用",
    project_rescan_applying: "应用中...",
    project_rescan_done: "应用成功。",
    project_rescan_close: "关闭",

    settings_agents: "AI 客户端",
    settings_agents_auto: "自动检测",
    settings_agents_manual: "手动配置",
    settings_agents_hint: "选择要同步技能的 AI 客户端。自动检测会检查本机上已安装的客户端。",
};

pub fn texts(lang: Language) -> &'static Texts {
    match lang {
        Language::English => &EN,
        Language::Chinese => &ZH,
    }
}
