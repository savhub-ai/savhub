use chrono::Utc;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use reqwest::Url;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::{NewUserRow, NewUserTokenRow, UserChangeset, UserRow};
use crate::schema::{user_tokens, users};
use crate::service::helpers::{db_conn, insert_audit_log};
use crate::state::app_state;

const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";
const GITHUB_SCOPE: &str = "read:user";
const GITHUB_STATE_COOKIE: &str = "savhub_github_state";
const GITHUB_RETURN_TO_COOKIE: &str = "savhub_github_return_to";
const GITHUB_TOKEN_NAME: &str = "github-oauth";
const GITHUB_COOKIE_MAX_AGE_SECS: i64 = 600;
const SAVHUB_USER_AGENT: &str = "savhub-server";

#[derive(Debug, Clone)]
pub struct AuthRedirect {
    pub location: String,
    pub set_cookies: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GithubAccessTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubUser {
    id: i64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
    bio: Option<String>,
}

pub fn start_login(return_to: Option<&str>) -> Result<AuthRedirect, AppError> {
    let config = &app_state().config;
    let return_to = validate_return_to(return_to.unwrap_or(config.frontend_origin.as_str()))?;
    let state = Uuid::now_v7().simple().to_string();

    let mut url = Url::parse(GITHUB_AUTHORIZE_URL).map_err(|error| {
        AppError::Internal(format!("failed to build GitHub authorize URL: {error}"))
    })?;
    url.query_pairs_mut()
        .append_pair("client_id", &config.github_client_id)
        .append_pair("redirect_uri", &config.github_redirect_url)
        .append_pair("scope", GITHUB_SCOPE)
        .append_pair("state", &state);

    Ok(AuthRedirect {
        location: url.to_string(),
        set_cookies: vec![
            build_cookie(GITHUB_STATE_COOKIE, &state, GITHUB_COOKIE_MAX_AGE_SECS),
            build_cookie(
                GITHUB_RETURN_TO_COOKIE,
                &hex_encode(return_to.as_str()),
                GITHUB_COOKIE_MAX_AGE_SECS,
            ),
        ],
    })
}

pub async fn finish_login(
    code: &str,
    state: &str,
    cookie_header: Option<&str>,
) -> Result<AuthRedirect, AppError> {
    let expected_state = cookie_value(cookie_header, GITHUB_STATE_COOKIE)
        .ok_or_else(|| AppError::Unauthorized("missing GitHub login state".to_string()))?;
    if expected_state != state {
        return Err(AppError::Unauthorized(
            "GitHub login state did not match".to_string(),
        ));
    }

    let return_to = load_return_to(cookie_header)?;
    let access_token = exchange_code_for_access_token(code).await?;
    let github_user = fetch_github_user(&access_token).await?;

    let mut conn = db_conn().await?;
    let user = upsert_github_user(&mut conn, &github_user).await?;
    let token = issue_github_token(&mut conn, user.id).await?;

    insert_audit_log(
        &mut conn,
        Some(user.id),
        "user.login",
        "user",
        Some(user.id),
        serde_json::json!({"github_login": github_user.login}),
    )
    .await?;

    Ok(AuthRedirect {
        location: build_success_redirect(&return_to, &token)?,
        set_cookies: clear_auth_cookies(),
    })
}

pub fn error_redirect(cookie_header: Option<&str>, message: &str) -> AuthRedirect {
    let location = load_return_to(cookie_header)
        .and_then(|return_to| build_error_redirect(&return_to, message))
        .unwrap_or_else(|_| fallback_error_redirect(message));
    AuthRedirect {
        location,
        set_cookies: clear_auth_cookies(),
    }
}

async fn exchange_code_for_access_token(code: &str) -> Result<String, AppError> {
    let config = &app_state().config;
    let client = reqwest::Client::new();
    let response = client
        .post(GITHUB_ACCESS_TOKEN_URL)
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, SAVHUB_USER_AGENT)
        .json(&serde_json::json!({
            "client_id": config.github_client_id,
            "client_secret": config.github_client_secret,
            "code": code,
            "redirect_uri": config.github_redirect_url,
        }))
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("GitHub token exchange failed: {error}")))?;

    let status = response.status();
    let payload = response
        .json::<GithubAccessTokenResponse>()
        .await
        .map_err(|error| {
            AppError::Internal(format!("failed to decode GitHub token response: {error}"))
        })?;

    if !status.is_success() {
        let detail = payload
            .error_description
            .or(payload.error)
            .unwrap_or_else(|| format!("GitHub returned HTTP {}", status.as_u16()));
        return Err(AppError::Unauthorized(detail));
    }

    if let Some(error) = payload.error {
        let detail = payload.error_description.unwrap_or(error);
        return Err(AppError::Unauthorized(detail));
    }

    payload
        .access_token
        .ok_or_else(|| AppError::Unauthorized("GitHub did not return an access token".to_string()))
}

async fn fetch_github_user(access_token: &str) -> Result<GithubUser, AppError> {
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static(SAVHUB_USER_AGENT));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|error| AppError::Internal(format!("invalid auth header: {error}")))?,
    );

    let response = client
        .get(GITHUB_USER_URL)
        .headers(headers)
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("GitHub user lookup failed: {error}")))?;

    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        let message = if detail.trim().is_empty() {
            format!("GitHub user lookup returned HTTP {}", status.as_u16())
        } else {
            format!("GitHub user lookup failed: {detail}")
        };
        return Err(AppError::Unauthorized(message));
    }

    response
        .json::<GithubUser>()
        .await
        .map_err(|error| AppError::Internal(format!("failed to decode GitHub user: {error}")))
}

async fn upsert_github_user(
    conn: &mut AsyncPgConnection,
    github_user: &GithubUser,
) -> Result<UserRow, AppError> {
    let github_user_id = github_user.id.to_string();
    let github_login = github_user.login.to_lowercase();
    let existing = find_existing_user(conn, &github_user_id, &github_login).await?;
    let role = login_role(&github_login, existing.as_ref());
    let is_new = existing.is_none();
    let now = Utc::now();
    tracing::info!(
        "GitHub user {} (id={}) -> role={}, new={}",
        github_login,
        github_user_id,
        role,
        is_new
    );

    if let Some(existing) = existing {
        diesel::update(users::table.find(existing.id))
            .set(UserChangeset {
                display_name: github_user.name.as_ref().map(|value| Some(value.clone())),
                bio: github_user.bio.as_ref().map(|value| Some(value.clone())),
                avatar_url: github_user
                    .avatar_url
                    .as_ref()
                    .map(|value| Some(value.clone())),
                github_user_id: Some(Some(github_user_id)),
                github_login: Some(Some(github_login)),
                role: Some(role),
                updated_at: Some(now),
                ..UserChangeset::default()
            })
            .execute(conn)
            .await?;

        return users::table
            .find(existing.id)
            .select(UserRow::as_select())
            .first(conn)
            .await
            .map_err(Into::into);
    }

    let user = NewUserRow {
        id: Uuid::now_v7(),
        handle: unique_handle(conn, &github_login).await?,
        display_name: github_user.name.clone(),
        bio: github_user.bio.clone(),
        avatar_url: github_user.avatar_url.clone(),
        github_user_id: Some(github_user_id),
        github_login: Some(github_login),
        role,
        created_at: now,
        updated_at: now,
    };

    diesel::insert_into(users::table)
        .values(&user)
        .execute(conn)
        .await?;

    users::table
        .find(user.id)
        .select(UserRow::as_select())
        .first(conn)
        .await
        .map_err(Into::into)
}

async fn find_existing_user(
    conn: &mut AsyncPgConnection,
    github_user_id: &str,
    github_login: &str,
) -> Result<Option<UserRow>, AppError> {
    if let Some(user) = users::table
        .filter(users::github_user_id.eq(Some(github_user_id.to_string())))
        .select(UserRow::as_select())
        .first::<UserRow>(conn)
        .await
        .optional()?
    {
        return Ok(Some(user));
    }

    if let Some(user) = users::table
        .filter(users::github_login.eq(Some(github_login.to_string())))
        .select(UserRow::as_select())
        .first::<UserRow>(conn)
        .await
        .optional()?
    {
        return Ok(Some(user));
    }

    users::table
        .filter(users::handle.eq(github_login))
        .filter(users::github_user_id.is_null())
        .filter(users::github_login.is_null())
        .select(UserRow::as_select())
        .first::<UserRow>(conn)
        .await
        .optional()
        .map_err(Into::into)
}

async fn unique_handle(conn: &mut AsyncPgConnection, base: &str) -> Result<String, AppError> {
    for suffix in 0..10_000 {
        let candidate = if suffix == 0 {
            base.to_string()
        } else {
            format!("{base}-{suffix}")
        };
        let exists = users::table
            .filter(users::handle.eq(&candidate))
            .select(UserRow::as_select())
            .first::<UserRow>(conn)
            .await
            .optional()?
            .is_some();
        if !exists {
            return Ok(candidate);
        }
    }

    Err(AppError::Conflict(
        "could not allocate a unique handle for the GitHub account".to_string(),
    ))
}

async fn issue_github_token(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
) -> Result<String, AppError> {
    // Keep previous tokens so other clients (CLI, desktop) stay logged in.
    let token = format!("ghu_{}", Uuid::now_v7().simple());
    let token_hash = super::helpers::hash_string(&token);
    let token_prefix: String = token.chars().take(12).collect();
    diesel::insert_into(user_tokens::table)
        .values(NewUserTokenRow {
            id: Uuid::now_v7(),
            user_id,
            name: GITHUB_TOKEN_NAME.to_string(),
            // The plaintext column is still present during B1 phase 1; it
            // will be dropped in the follow-up migration. New rows still
            // populate it so a rollback to the old auth.rs lookup works.
            token: token.clone(),
            created_at: Utc::now(),
            token_hash,
            token_prefix,
        })
        .execute(conn)
        .await?;
    Ok(token)
}

fn login_role(github_login: &str, existing: Option<&UserRow>) -> String {
    let config = &app_state().config;
    if config
        .github_admin_logins
        .iter()
        .any(|login| login == github_login)
    {
        return "admin".to_string();
    }
    if config
        .github_moderator_logins
        .iter()
        .any(|login| login == github_login)
    {
        return "moderator".to_string();
    }
    existing
        .map(|user| user.role.clone())
        .unwrap_or_else(|| "user".to_string())
}

fn validate_return_to(value: &str) -> Result<Url, AppError> {
    let config = &app_state().config;
    let return_to = Url::parse(value)
        .map_err(|_| AppError::BadRequest("return_to must be an absolute URL".to_string()))?;
    let frontend_origin = Url::parse(&config.frontend_origin)
        .map_err(|error| AppError::Internal(format!("invalid SAVHUB_FRONTEND_ORIGIN: {error}")))?;

    if same_origin(&return_to, &frontend_origin) || is_loopback_url(&return_to) {
        return Ok(return_to);
    }

    Err(AppError::Forbidden(
        "return_to must point to the configured frontend or a loopback address".to_string(),
    ))
}

fn load_return_to(cookie_header: Option<&str>) -> Result<Url, AppError> {
    let value = cookie_value(cookie_header, GITHUB_RETURN_TO_COOKIE).ok_or_else(|| {
        AppError::Unauthorized("missing GitHub login redirect target".to_string())
    })?;
    let decoded = hex_decode(&value).ok_or_else(|| {
        AppError::Unauthorized("invalid GitHub login redirect target".to_string())
    })?;
    validate_return_to(&decoded)
}

fn build_success_redirect(return_to: &Url, token: &str) -> Result<String, AppError> {
    build_redirect(return_to, "auth_token", token)
}

fn build_error_redirect(return_to: &Url, message: &str) -> Result<String, AppError> {
    build_redirect(return_to, "auth_error", message)
}

fn build_redirect(return_to: &Url, key: &str, value: &str) -> Result<String, AppError> {
    let mut url = return_to.clone();
    if is_loopback_url(&url) {
        url.query_pairs_mut().append_pair(key, value);
    } else {
        url.set_fragment(Some(&format!("{key}={value}")));
    }
    Ok(url.to_string())
}

fn fallback_error_redirect(message: &str) -> String {
    let config = &app_state().config;
    let mut url = Url::parse(&config.frontend_origin)
        .unwrap_or_else(|_| Url::parse("http://127.0.0.1:8081").expect("static URL is valid"));
    url.set_fragment(Some(&format!("auth_error={message}")));
    url.to_string()
}

fn cookie_value(cookie_header: Option<&str>, name: &str) -> Option<String> {
    cookie_header.and_then(|header| {
        header
            .split(';')
            .filter_map(|cookie| cookie.trim().split_once('='))
            .find_map(|(cookie_name, value)| (cookie_name == name).then(|| value.to_string()))
    })
}

fn build_cookie(name: &str, value: &str, max_age_secs: i64) -> String {
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}")
}

fn clear_auth_cookies() -> Vec<String> {
    vec![
        build_cookie(GITHUB_STATE_COOKIE, "", 0),
        build_cookie(GITHUB_RETURN_TO_COOKIE, "", 0),
    ]
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn is_loopback_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
}

fn hex_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        encoded.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        encoded.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    encoded
}

fn hex_decode(value: &str) -> Option<String> {
    if !value.len().is_multiple_of(2) {
        return None;
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    let mut chars = value.chars();
    while let (Some(high), Some(low)) = (chars.next(), chars.next()) {
        let high = high.to_digit(16)?;
        let low = low.to_digit(16)?;
        bytes.push(((high << 4) | low) as u8);
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_preserves_urls() {
        let url = "http://127.0.0.1:4567/callback?x=1&y=2";
        let decoded = hex_decode(&hex_encode(url)).expect("hex decode");
        assert_eq!(decoded, url);
    }

    #[test]
    fn loopback_redirect_uses_query_parameters() {
        let url = Url::parse("http://127.0.0.1:3456/callback").expect("url");
        let redirect = build_redirect(&url, "auth_token", "abc 123").expect("redirect");
        assert_eq!(
            redirect,
            "http://127.0.0.1:3456/callback?auth_token=abc+123"
        );
    }

    #[test]
    fn cookie_parser_finds_named_cookie() {
        let header = "a=1; savhub_github_state=xyz; b=2";
        assert_eq!(
            cookie_value(Some(header), GITHUB_STATE_COOKIE),
            Some("xyz".to_string())
        );
    }
}
