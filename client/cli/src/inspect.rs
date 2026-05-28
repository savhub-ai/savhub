//! `savhub inspect` — print or JSON-dump skill metadata, version history,
//! file manifest, and (optionally) the raw content of a single file at a
//! pinned version. Extracted from `main.rs` together with its three
//! single-callsite helpers.

use anyhow::{Result, anyhow, bail};
use savhub_local::api::ApiClient;
use savhub_local::skills::inspect_zip;
use savhub_shared::{FileContentResponse, SkillDetailResponse};
use serde_json::json;

use crate::{GlobalOpts, InspectArgs, normalize_slug, optional_client, truncate};

pub(crate) async fn cmd_inspect(opts: &GlobalOpts, args: InspectArgs) -> Result<()> {
    if args.version.is_some() && args.tag.is_some() {
        bail!("use either --version or --tag");
    }
    let slug = normalize_slug(&args.slug)?;
    let client = optional_client(opts)?;
    let detail = client
        .get_json::<SkillDetailResponse>(&format!("/skills/{slug}"))
        .await?;
    let selected_version =
        resolve_selected_version(&detail, args.version.as_deref(), args.tag.as_deref())?;

    let file_payload = if let Some(path) = args.file.as_deref() {
        let mut url = client.v1_url(&format!("/skills/{slug}/file"))?;
        url.query_pairs_mut().append_pair("path", path);
        if let Some(version) = selected_version.as_deref() {
            url.query_pairs_mut().append_pair("version", version);
        }
        Some(client.get_json_url::<FileContentResponse>(url).await?)
    } else {
        None
    };

    let selected_files = if args.files {
        match (
            selected_version.as_deref(),
            detail
                .latest_version
                .as_ref()
                .map(|value| value.version.as_str()),
        ) {
            (None, _) => Some(latest_files_json(&detail)),
            (Some(requested), Some(current)) if requested == current => {
                Some(latest_files_json(&detail))
            }
            (Some(requested), _) => {
                let bytes = download_skill_bundle(&client, &slug, Some(requested), None).await?;
                Some(
                    inspect_zip(&bytes)?
                        .into_iter()
                        .map(|file| {
                            json!({
                                "path": file.path,
                                "size": file.size,
                                "sha256": file.sha256,
                            })
                        })
                        .collect::<Vec<_>>(),
                )
            }
        }
    } else {
        None
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "detail": detail,
                "selected_version": selected_version,
                "file": file_payload,
                "files": selected_files,
            }))?
        );
        return Ok(());
    }

    println!("{}  {}", detail.skill.slug, detail.skill.display_name);
    if let Some(summary) = detail.skill.summary.as_deref() {
        println!("Summary: {summary}");
    }
    println!("Owner: @{}", detail.skill.owner.handle);
    println!(
        "Latest: {}",
        detail
            .latest_version
            .as_ref()
            .map(|value| value.version.as_str())
            .unwrap_or("?")
    );
    println!(
        "Stats: {} downloads, {} stars, {} installs, {} users, {} versions, {} comments",
        detail.skill.stats.downloads,
        detail.skill.stats.stars,
        detail.skill.stats.installs,
        detail.skill.stats.unique_users,
        detail.skill.stats.versions,
        detail.skill.stats.comments
    );
    println!("Moderation: {:?}", detail.skill.moderation_status);
    if !detail.skill.tags.is_empty() {
        println!(
            "Tags: {}",
            detail
                .skill
                .tags
                .iter()
                .map(|(tag, version)| format!("{tag}={version}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if let Some(version) = selected_version.as_deref() {
        println!("Selected: {version}");
    }

    if args.versions {
        let limit = args.limit.unwrap_or(25);
        println!("Versions:");
        for entry in detail.versions.iter().take(limit) {
            println!(
                "  {}  {}  {}",
                entry.version,
                entry.created_at.to_rfc3339(),
                truncate(&entry.changelog, 80)
            );
        }
    }

    if let Some(files) = selected_files {
        if files.is_empty() {
            println!("Files: none");
        } else {
            println!("Files:");
            for file in files {
                println!(
                    "  {}  {}  {}",
                    file.get("path")
                        .and_then(|value| value.as_str())
                        .unwrap_or("?"),
                    file.get("size")
                        .and_then(|value| value.as_i64())
                        .unwrap_or_default(),
                    file.get("sha256")
                        .and_then(|value| value.as_str())
                        .unwrap_or("?")
                );
            }
        }
    }

    if let Some(file) = file_payload {
        println!();
        println!("{}:", file.path);
        print!("{}", file.content);
        if !file.content.ends_with('\n') {
            println!();
        }
    }

    Ok(())
}

fn resolve_selected_version(
    detail: &SkillDetailResponse,
    version: Option<&str>,
    tag: Option<&str>,
) -> Result<Option<String>> {
    if let Some(version) = version {
        return Ok(Some(version.to_string()));
    }
    if let Some(tag) = tag {
        return detail
            .skill
            .tags
            .get(tag)
            .map(|value| Some(value.clone()))
            .ok_or_else(|| anyhow!("unknown tag `{tag}`"));
    }
    Ok(detail
        .latest_version
        .as_ref()
        .map(|value| value.version.clone()))
}

fn latest_files_json(detail: &SkillDetailResponse) -> Vec<serde_json::Value> {
    detail
        .latest_version
        .as_ref()
        .map(|value| {
            value
                .files
                .iter()
                .map(|file| {
                    json!({
                        "path": file.path,
                        "size": file.size,
                        "sha256": file.sha256,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

async fn download_skill_bundle(
    client: &ApiClient,
    slug: &str,
    version: Option<&str>,
    tag: Option<&str>,
) -> Result<Vec<u8>> {
    let mut url = client.v1_url("/download")?;
    url.query_pairs_mut()
        .append_pair("slug", slug)
        .append_pair("kind", "skill");
    if let Some(version) = version {
        url.query_pairs_mut().append_pair("version", version);
    }
    if let Some(tag) = tag {
        url.query_pairs_mut().append_pair("tag", tag);
    }
    client.get_bytes_url(url).await
}
