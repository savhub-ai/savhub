//! Login / logout / whoami commands.
//!
//! The OAuth callback server lives in `oauth.rs`; this module is just the
//! command-level glue that drives it and persists the resulting token to
//! the global config.

use std::net::TcpListener;

use anyhow::{Context, Result, bail};
use savhub_local::api::ApiClient;
use savhub_local::config::{read_global_config, write_global_config};
use savhub_shared::WhoAmIResponse;

use crate::{GlobalOpts, LoginArgs, authed_client, oauth};

pub(crate) async fn cmd_login(opts: &GlobalOpts, args: LoginArgs) -> Result<()> {
    if args.token.is_some() {
        bail!(
            "manual token login is no longer supported; run `savhub login` and complete GitHub auth in the browser"
        );
    }
    if args.label.is_some() {
        eprintln!("Ignoring --label; savhub login now uses GitHub OAuth.");
    }

    let client = ApiClient::new(&opts.api_base, None);
    let listener = TcpListener::bind("127.0.0.1:0")
        .context("failed to bind a local callback port for GitHub login")?;
    let return_to = format!(
        "http://127.0.0.1:{}/callback",
        listener
            .local_addr()
            .context("failed to resolve local callback address")?
            .port()
    );
    let mut login_url = client.v1_url("/auth/github/start")?;
    login_url
        .query_pairs_mut()
        .append_pair("return_to", &return_to);

    if args.no_browser {
        println!("Open this URL in your browser to finish GitHub login:\n{login_url}");
    } else if let Err(error) = oauth::open_browser(login_url.as_str()) {
        eprintln!(
            "Failed to open a browser automatically: {error}\nOpen this URL manually:\n{login_url}"
        );
    }

    let token = oauth::wait_for_login_callback(listener)?;
    let client = ApiClient::new(&opts.api_base, Some(token.clone()));
    let whoami = client.get_json::<WhoAmIResponse>("/whoami").await?;
    let Some(user) = whoami.user else {
        bail!("login failed: token is not valid");
    };
    let mut existing = read_global_config()?.unwrap_or_default();
    existing.api_base = Some(opts.api_base.clone());
    existing.token = Some(token);
    write_global_config(&existing)?;
    println!("Logged in as @{} via GitHub", user.handle);
    Ok(())
}

pub(crate) fn cmd_logout(_opts: &GlobalOpts) -> Result<()> {
    let mut existing = read_global_config()?.unwrap_or_default();
    existing.token = None;
    write_global_config(&existing)?;
    println!("Logged out locally.");
    Ok(())
}

pub(crate) async fn cmd_whoami(opts: &GlobalOpts) -> Result<()> {
    let client = authed_client(opts)?;
    let whoami = client.get_json::<WhoAmIResponse>("/whoami").await?;
    let Some(user) = whoami.user else {
        bail!("token is valid but no user is associated with it");
    };
    let token_name = whoami
        .token_name
        .as_deref()
        .map(|value| format!(" via {}", value))
        .unwrap_or_default();
    println!("{}{}", user.handle, token_name);
    Ok(())
}
