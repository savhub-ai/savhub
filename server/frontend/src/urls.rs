//! Pure string helpers for displaying and normalising git/repo URLs.
//! Extracted from `app.rs` to keep that file focused on Dioxus components.

/// Derive a human-readable repo slug from git_url
/// (e.g. `https://github.com/org/repo.git` → `github.com/org/repo`).
pub fn derive_repo_slug(git_url: &str) -> String {
    strip_url_scheme(git_url).to_string()
}

/// Strip `https://` / `http://` prefix and `.git` suffix for display.
pub fn strip_url_scheme(url: &str) -> &str {
    let url = url.trim().trim_end_matches('/');
    let url = url.trim_end_matches(".git");
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}

/// Strip only `https://` / `http://` prefix, keeping `.git` suffix intact.
pub fn strip_url_scheme_keep_git(url: &str) -> &str {
    let url = url.trim().trim_end_matches('/');
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scheme_and_git_suffix() {
        assert_eq!(
            strip_url_scheme("https://github.com/org/repo.git"),
            "github.com/org/repo"
        );
        assert_eq!(
            strip_url_scheme("http://example.com/foo/"),
            "example.com/foo"
        );
        assert_eq!(strip_url_scheme("git@host:path"), "git@host:path");
    }

    #[test]
    fn keeps_git_suffix_in_keep_variant() {
        assert_eq!(
            strip_url_scheme_keep_git("https://github.com/org/repo.git"),
            "github.com/org/repo.git"
        );
    }

    #[test]
    fn derive_slug_drops_scheme_and_suffix() {
        assert_eq!(
            derive_repo_slug("https://github.com/savhub-ai/savhub.git"),
            "github.com/savhub-ai/savhub"
        );
    }
}
