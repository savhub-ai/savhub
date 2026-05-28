use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use regex::Regex;

use crate::error::AppError;
use crate::models::IndexRuleRow;
use crate::schema::index_rules;
use crate::service::helpers::normalize_git_url;

/// The resolved indexing strategy for a given git URL + subdir.
/// Used during flock generation to determine how skills are grouped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexStrategy {
    /// Every matched directory becomes its own flock.
    EachDirAsFlock,
    /// Default LCA-based smart grouping.
    Smart,
}

/// Result of resolving index rules: which strategy to use and where to scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRule {
    pub strategy: IndexStrategy,
    /// The scan path inside the repo. `"."` means scan from root.
    pub scan_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleMatch {
    resolved: ResolvedRule,
    score: i32,
}

/// Look up the best-matching index rule for the given git URL and subdir,
/// and return the strategy + scan path to use during flock generation.
///
/// The `index_rules` table has two columns for matching:
/// - `repo_url`: normalized git URL (e.g. `https://github.com/org/repo`)
/// - `path_regex`: concrete path, wildcard `*`, or regex matched against the subdir path
///
/// When the user submits a job, we find all rules for that repo, then
/// pick the best regex match against the subdir. The matched rule's
/// strategy determines how skills are grouped into flocks.
pub async fn resolve_index_rule(
    conn: &mut AsyncPgConnection,
    git_url: &str,
    subdir: &str,
) -> Result<ResolvedRule, AppError> {
    let normalized_url = normalize_git_url(git_url);
    let subdir_normalized = normalize_subdir(subdir);

    println!(
        "[index_rules] resolve: git_url='{}' -> normalized='{}', subdir='{}'",
        git_url, normalized_url, subdir_normalized
    );

    let rules = index_rules::table
        .filter(index_rules::repo_url.eq(&normalized_url))
        .select(IndexRuleRow::as_select())
        .load::<IndexRuleRow>(conn)
        .await?;

    println!(
        "[index_rules] found {} rule(s) for '{}'",
        rules.len(),
        normalized_url
    );
    for rule in &rules {
        println!(
            "[index_rules]   rule id={}, path_regex='{}', strategy='{}'",
            rule.id, rule.path_regex, rule.strategy
        );
    }

    // Also dump all rules in the table for diagnostics
    let all_rules = index_rules::table
        .select(IndexRuleRow::as_select())
        .load::<IndexRuleRow>(conn)
        .await
        .unwrap_or_default();
    println!("[index_rules] total rules in DB: {}", all_rules.len());
    for r in &all_rules {
        println!(
            "[index_rules]   db: repo_url='{}', path_regex='{}', strategy='{}'",
            r.repo_url, r.path_regex, r.strategy
        );
    }

    if rules.is_empty() {
        println!(
            "[index_rules] no rules matched for '{}', using Smart with subdir='{}'",
            normalized_url, subdir_normalized
        );
        return Ok(ResolvedRule {
            strategy: IndexStrategy::Smart,
            scan_path: subdir_normalized,
        });
    }

    match resolve_rule_from_rows(&rules, &subdir_normalized) {
        Some(resolved) => Ok(resolved),
        None => {
            tracing::debug!(
                subdir = %subdir_normalized,
                "no index rule matched, using Smart"
            );
            Ok(ResolvedRule {
                strategy: IndexStrategy::Smart,
                scan_path: subdir_normalized,
            })
        }
    }
}

fn parse_strategy(value: &str) -> IndexStrategy {
    match value.trim() {
        "each_dir_as_flock" | "subdirs_as_flocks" => IndexStrategy::EachDirAsFlock,
        _ => IndexStrategy::Smart,
    }
}

/// Normalize a subdir: "." and "" both become ".", trim slashes.
fn normalize_subdir(subdir: &str) -> String {
    let trimmed = subdir.trim().trim_matches('/');
    if trimmed.is_empty() {
        ".".to_string()
    } else {
        trimmed.to_string()
    }
}

fn resolve_rule_from_rows(rules: &[IndexRuleRow], subdir_normalized: &str) -> Option<ResolvedRule> {
    rules
        .iter()
        .filter_map(|rule| match_rule(rule, subdir_normalized))
        .max_by_key(|matched| matched.score)
        .map(|matched| matched.resolved)
}

fn match_rule(rule: &IndexRuleRow, subdir_normalized: &str) -> Option<RuleMatch> {
    let pattern = rule.path_regex.trim();
    if pattern.is_empty() {
        return None;
    }

    if is_wildcard_rule(pattern) {
        println!(
            "Testing wildcard pattern '{}' against subdir '{}'   matched: true",
            pattern, subdir_normalized
        );
        return Some(RuleMatch {
            resolved: ResolvedRule {
                strategy: parse_strategy(&rule.strategy),
                scan_path: subdir_normalized.to_string(),
            },
            score: 1000,
        });
    }

    if is_concrete_rule_path(pattern) {
        let rule_path = normalize_subdir(pattern);
        let matched = subdir_normalized == "." || subdir_normalized == rule_path;
        println!(
            "Testing concrete path '{}' against subdir '{}'   matched: {}",
            rule_path, subdir_normalized, matched
        );
        if !matched {
            return None;
        }

        let score = if subdir_normalized == rule_path {
            4000 + rule_path.len() as i32
        } else {
            3000 + rule_path.len() as i32
        };

        return Some(RuleMatch {
            resolved: ResolvedRule {
                strategy: parse_strategy(&rule.strategy),
                scan_path: rule_path,
            },
            score,
        });
    }

    let re = match Regex::new(pattern) {
        Ok(re) => re,
        Err(e) => {
            tracing::warn!(pattern = %pattern, error = %e, "invalid path_regex, skipping rule");
            return None;
        }
    };

    let matched = re.is_match(subdir_normalized);
    println!(
        "Testing regex pattern '{}' against subdir '{}'   matched: {}",
        pattern, subdir_normalized, matched
    );
    tracing::debug!(
        pattern = %pattern,
        subdir = %subdir_normalized,
        matched = matched,
        "testing rule"
    );

    matched.then(|| RuleMatch {
        resolved: ResolvedRule {
            strategy: parse_strategy(&rule.strategy),
            scan_path: subdir_normalized.to_string(),
        },
        score: 2000 + pattern.len() as i32,
    })
}

fn is_wildcard_rule(pattern: &str) -> bool {
    pattern.trim() == "*"
}

fn is_concrete_rule_path(pattern: &str) -> bool {
    !pattern.is_empty()
        && !is_wildcard_rule(pattern)
        && !pattern.chars().any(|ch| {
            matches!(
                ch,
                '\\' | '^' | '$' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;

    fn rule(path_regex: &str, strategy: &str) -> IndexRuleRow {
        IndexRuleRow {
            id: Uuid::now_v7(),
            repo_url: "https://github.com/example/repo.git".to_string(),
            path_regex: path_regex.to_string(),
            strategy: strategy.to_string(),
            description: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn normalize_subdir_root() {
        assert_eq!(normalize_subdir("."), ".");
        assert_eq!(normalize_subdir(""), ".");
        assert_eq!(normalize_subdir("/"), ".");
    }

    #[test]
    fn normalize_subdir_path() {
        assert_eq!(normalize_subdir("skills"), "skills");
        assert_eq!(normalize_subdir("skills/"), "skills");
        assert_eq!(normalize_subdir("/skills/"), "skills");
    }

    #[test]
    fn wildcard_rule_matches_root_subdir() {
        let rules = vec![rule("*", "each_dir_as_flock")];

        let resolved = resolve_rule_from_rows(&rules, ".").expect("rule should match");

        assert_eq!(
            resolved,
            ResolvedRule {
                strategy: IndexStrategy::EachDirAsFlock,
                scan_path: ".".to_string(),
            }
        );
    }

    #[test]
    fn concrete_rule_overrides_root_scan_path() {
        let rules = vec![rule("skills", "each_dir_as_flock")];

        let resolved = resolve_rule_from_rows(&rules, ".").expect("rule should match");

        assert_eq!(
            resolved,
            ResolvedRule {
                strategy: IndexStrategy::EachDirAsFlock,
                scan_path: "skills".to_string(),
            }
        );
    }

    #[test]
    fn concrete_rule_matches_exact_subdir() {
        let rules = vec![rule("skills", "each_dir_as_flock")];

        let resolved = resolve_rule_from_rows(&rules, "skills").expect("rule should match");

        assert_eq!(
            resolved,
            ResolvedRule {
                strategy: IndexStrategy::EachDirAsFlock,
                scan_path: "skills".to_string(),
            }
        );
    }

    #[test]
    fn concrete_rule_beats_wildcard_for_root_scan() {
        let rules = vec![rule("*", "smart"), rule("skills", "each_dir_as_flock")];

        let resolved = resolve_rule_from_rows(&rules, ".").expect("rule should match");

        assert_eq!(resolved.strategy, IndexStrategy::EachDirAsFlock);
        assert_eq!(resolved.scan_path, "skills");
    }

    #[test]
    fn regex_rule_matches_existing_subdir_without_overriding_scan_root() {
        let rules = vec![rule("^src/.+$", "each_dir_as_flock")];

        let resolved =
            resolve_rule_from_rows(&rules, "src/skills").expect("regex rule should match");

        assert_eq!(
            resolved,
            ResolvedRule {
                strategy: IndexStrategy::EachDirAsFlock,
                scan_path: "src/skills".to_string(),
            }
        );
    }

    #[test]
    fn unmatched_concrete_rule_returns_none() {
        let rules = vec![rule("skills", "each_dir_as_flock")];

        let resolved = resolve_rule_from_rows(&rules, "examples");

        assert!(resolved.is_none());
    }
}
