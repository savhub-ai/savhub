//! GitHub OAuth callback handling for `savhub login`.
//!
//! Spins up a tiny single-shot HTTP server on a loopback port, redirects the
//! browser there after GitHub auth completes, parses the `auth_token` from
//! the redirect query, and renders an inline status page.
//!
//! Extracted from `main.rs` so the login command is the only thing that needs
//! to know about the local listener/HTML response details.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

pub(crate) fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        ProcessCommand::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map(|_| ())
            .map_err(|error| anyhow!("failed to launch browser: {error}"))?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        ProcessCommand::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| anyhow!("failed to launch browser: {error}"))?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        ProcessCommand::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| anyhow!("failed to launch browser: {error}"))?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(anyhow!(
        "automatic browser launch is not supported on this platform"
    ))
}

pub(crate) fn wait_for_login_callback(listener: TcpListener) -> Result<String> {
    listener
        .set_nonblocking(true)
        .context("failed to configure local callback listener")?;
    let deadline = Instant::now() + Duration::from_secs(240);

    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Some(token) = handle_login_callback(&mut stream)? {
                    return Ok(token);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    bail!("timed out waiting for the GitHub login callback");
                }
                thread::sleep(Duration::from_millis(150));
            }
            Err(error) => return Err(anyhow!("failed to accept login callback: {error}")),
        }
    }
}

fn handle_login_callback(stream: &mut TcpStream) -> Result<Option<String>> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .context("failed to read the login callback stream")?,
    );
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("failed to read the login callback request line")?;

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    if !path.starts_with("/callback") {
        write_callback_page(
            stream,
            "Savhub login did not recognize the callback path. You can close this window.",
            true,
        )?;
        return Ok(None);
    }

    let url = reqwest::Url::parse(&format!("http://127.0.0.1{path}"))
        .context("failed to parse the login callback URL")?;
    let mut auth_token = None;
    let mut auth_error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "auth_token" => auth_token = Some(value.into_owned()),
            "auth_error" => auth_error = Some(value.into_owned()),
            _ => {}
        }
    }

    if let Some(error) = auth_error {
        write_callback_page(
            stream,
            "Savhub login failed. Return to the terminal for details.",
            true,
        )?;
        bail!("GitHub login failed: {error}");
    }

    if let Some(token) = auth_token {
        write_callback_page(
            stream,
            "Savhub login is complete. You can close this window.",
            false,
        )?;
        return Ok(Some(token));
    }

    write_callback_page(
        stream,
        "Savhub login is still waiting for an authentication result.",
        true,
    )?;
    Ok(None)
}

fn write_callback_page(stream: &mut TcpStream, message: &str, is_error: bool) -> Result<()> {
    let title = if is_error {
        "Login Failed"
    } else {
        "Login Complete"
    };
    let accent = if is_error { "#c0392b" } else { "#287850" };
    let body = format!(
        r##"<!doctype html><html><head><meta charset="utf-8"><title>Savhub — {title}</title>
<style>
body{{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;font-family:'Segoe UI',system-ui,sans-serif;background:#f6efe4;color:#2d2015}}
.card{{text-align:center;background:#fff;border-radius:16px;padding:48px 40px;box-shadow:0 2px 24px rgba(0,0,0,.08);max-width:400px}}
.logo{{width:72px;height:72px;margin:0 auto 20px}}
h1{{font-size:22px;margin:0 0 8px;color:{accent}}}
p{{font-size:15px;color:#5a4e42;margin:0;line-height:1.5}}
</style></head><body><div class="card">
<svg class="logo" viewBox="0 0 1021 1021" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><defs><linearGradient id="g0" x1="1496" y1="1345" x2="1547" y2="1022" gradientUnits="userSpaceOnUse" gradientTransform="matrix(.751 0 0 .781 -298 -272)"><stop offset="0" stop-color="#1D1E1F"/><stop offset="1" stop-color="#4C5154"/></linearGradient><linearGradient id="g1" x1="757" y1="719" x2="756" y2="438" gradientUnits="userSpaceOnUse" gradientTransform="matrix(.751 0 0 .781 -306 -290)"><stop offset="0" stop-color="#202122"/><stop offset="1" stop-color="#4B4F53"/></linearGradient></defs><path id="a" d="m1020 262c0 153 0 83 1 337-15-34-30-57-57-83C912 471 859 452 725 442 624 338 636 342 474 289c3-77 12-147 68-205 48-50 114-79 184-81 76-3 150 24 206 74 54 48 86 115 87 185z" style="stroke-width:.766"/><use href="#a" fill="#287850"/><use href="#a" transform="rotate(90 511 511)" fill="#0a0a0a"/><use href="#a" transform="rotate(180 511 511)" fill="#287850"/><use href="#a" transform="rotate(-90 510 512)" fill="#0a0a0a"/><path fill="url(#g0)" d="m773 544c18-18 44-29 69-28 30 1 58 17 78 40 19 21 32 47 36 75 4 10 3 36 1 46-5 33-22 63-48 82-51 39-118 26-155-27-21-31-29-69-23-106 5-28 18-65 42-82z" style="stroke-width:.766"/><path fill="url(#g1)" d="m116 163c0-4-1-8-1-13C121 21 298-9 375 70c17 17 24 32 31 55 6 31 1 57-16 84-48 76-160 83-228 34-23-17-41-43-46-72 0-2-1-5-1-7z" style="stroke-width:.766"/><circle cx="510" cy="510" r="232" fill="#fff"/></svg>
<h1>{title}</h1><p>{message}</p></div></body></html>"##
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .context("failed to write the login callback response")?;
    stream.flush().ok();
    Ok(())
}
