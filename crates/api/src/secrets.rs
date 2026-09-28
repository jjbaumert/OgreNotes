// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Secrets from AWS SSM Parameter Store.
//!
//! A self-hosted server keeps only its AWS credentials on disk; every
//! other secret (`JWT_SECRET`, `OAUTH_CLIENT_SECRET`, `SMTP_PASSWORD`,
//! `ANTHROPIC_API_KEY`, `MFA_ENCRYPTION_KEY`, …) lives in SSM as a
//! SecureString under one path, e.g. `/ogrenotes/home/secrets/`. When
//! `SECRETS_SSM_PATH` names that path, each parameter directly under it
//! becomes the environment variable of the same name — unless the
//! environment already sets it, so a local override always wins.
//!
//! This runs before the async runtime starts ([`load_into_env`]):
//! `AppConfig::from_env` reads the environment, and setting environment
//! variables is only sound while no other thread can be reading them.

use std::time::Duration;

/// The environment variable naming the SSM path. Unset: nothing is loaded.
pub const PATH_VAR: &str = "SECRETS_SSM_PATH";

/// Tries before giving up — the server may be booting before its network
/// is up. Without its secrets it can't start, so the last failure is fatal
/// (and a supervisor restarts it).
const ATTEMPTS: u32 = 6;
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// Load the secrets under `$SECRETS_SSM_PATH` into the environment.
///
/// Must be called from `main` before any other thread exists (before the
/// tokio runtime is built): it sets process environment variables.
pub fn load_into_env() {
    let Ok(path) = std::env::var(PATH_VAR) else { return };
    let path = normalize_path(&path);
    // A throwaway single-threaded runtime just for the fetch. It is dropped
    // before any variable is set, so no runtime thread is left running.
    let fetched = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("secrets: build fetch runtime");
        rt.block_on(fetch_with_retries(&path))
    };
    let pairs = fetched.unwrap_or_else(|e| {
        panic!("secrets: could not load parameters under {path} from SSM: {e}")
    });
    let mut loaded = Vec::new();
    for (name, value) in pairs {
        if std::env::var_os(&name).is_some() {
            continue;
        }
        // SAFETY: called from `main` before the async runtime or any other
        // thread exists (the fetch runtime above has been dropped), so no
        // other thread can be reading the environment concurrently.
        unsafe { std::env::set_var(&name, value) };
        loaded.push(name);
    }
    // Names only — never values. Logging isn't initialized yet.
    eprintln!("secrets: loaded {} from {path}: {}", loaded.len(), loaded.join(", "));
}

async fn fetch_with_retries(path: &str) -> Result<Vec<(String, String)>, String> {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .timeout_config(crate::storage_health::sdk_timeouts())
        .load()
        .await;
    let client = aws_sdk_ssm::Client::new(&config);
    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        match fetch(&client, path).await {
            Ok(pairs) => return Ok(pairs),
            Err(e) => {
                eprintln!("secrets: attempt {attempt}/{ATTEMPTS} failed: {e}");
                last = e;
                if attempt < ATTEMPTS {
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }
    Err(last)
}

/// Every parameter directly under `path`, decrypted, as `(env name, value)`.
async fn fetch(client: &aws_sdk_ssm::Client, path: &str) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let page = client
            .get_parameters_by_path()
            .path(path)
            .recursive(false)
            .with_decryption(true)
            .set_next_token(next)
            .send()
            .await
            // The full chain: a connection failure is only "unhandled
            // error" at the top level.
            .map_err(|e| aws_sdk_ssm::error::DisplayErrorContext(&e).to_string())?;
        for p in page.parameters.unwrap_or_default() {
            let (Some(name), Some(value)) = (p.name, p.value) else { continue };
            match env_name(path, &name) {
                Some(env) => pairs.push((env, value)),
                None => eprintln!("secrets: skipping {name}: not a valid environment variable name"),
            }
        }
        next = page.next_token;
        if next.is_none() {
            return Ok(pairs);
        }
    }
}

/// `path` with exactly one trailing `/` (SSM paths are `/a/b/`).
fn normalize_path(path: &str) -> String {
    format!("{}/", path.trim_end_matches('/'))
}

/// The environment variable a parameter maps to: its name relative to
/// `path`, when that is a plain `UPPER_SNAKE` name.
fn env_name(path: &str, parameter: &str) -> Option<String> {
    let rest = parameter.strip_prefix(path)?;
    let valid = !rest.is_empty()
        && !rest.starts_with(|c: char| c.is_ascii_digit())
        && rest.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_names_map_to_env_names() {
        let path = normalize_path("/ogrenotes/home/secrets");
        assert_eq!(path, "/ogrenotes/home/secrets/");
        assert_eq!(env_name(&path, "/ogrenotes/home/secrets/JWT_SECRET").as_deref(), Some("JWT_SECRET"));
        assert_eq!(env_name(&path, "/ogrenotes/home/secrets/SMTP_PASSWORD").as_deref(), Some("SMTP_PASSWORD"));
        // Not under the path, nested, lowercase, or otherwise not a plain name.
        for bad in [
            "/other/JWT_SECRET",
            "/ogrenotes/home/secrets/nested/JWT_SECRET",
            "/ogrenotes/home/secrets/jwt_secret",
            "/ogrenotes/home/secrets/9LIVES",
            "/ogrenotes/home/secrets/",
            "/ogrenotes/home/secrets/A-B",
        ] {
            assert_eq!(env_name(&path, bad), None, "{bad}");
        }
    }
}
