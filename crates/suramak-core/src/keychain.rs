use crate::SuramakError;

/// Retrieves the OpenAI API key from the OS keychain (macOS/Windows) or environment.
///
/// On Linux, only the `SURAMAK_OPENAI_API_KEY` environment variable is consulted.
pub fn get_api_key() -> Result<String, SuramakError> {
    // Try env var first on all platforms.
    if let Ok(key) = std::env::var("SURAMAK_OPENAI_API_KEY") {
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // Smoke harnesses must never reach the OS keychain. On a headless macOS CI
    // runner the keychain access prompt can never be answered, so the read
    // blocks forever and the app looks hung (the M2 scripted smoke: last log
    // line `get password from entry`, then silence until the 150 s kill).
    // In smoke mode polish simply reads as "not configured".
    if in_smoke_mode() {
        return Err(SuramakError::ApiKeyNotSet);
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let entry = keyring::Entry::new("Suramak", "polish")
            .map_err(|e| SuramakError::Keychain(e.to_string()))?;
        match entry.get_password() {
            Ok(pw) if !pw.is_empty() => return Ok(pw),
            Ok(_) => {}
            Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(SuramakError::Keychain(e.to_string())),
        }
    }

    Err(SuramakError::ApiKeyNotSet)
}

/// Stores the OpenAI API key in the OS keychain (macOS/Windows only).
///
/// On Linux this is a no-op because the keyring crate is not available;
/// use the `SURAMAK_OPENAI_API_KEY` environment variable instead.
pub fn set_api_key(key: &str) -> Result<(), SuramakError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let entry = keyring::Entry::new("Suramak", "polish")
            .map_err(|e| SuramakError::Keychain(e.to_string()))?;
        entry
            .set_password(key)
            .map_err(|e| SuramakError::Keychain(e.to_string()))?;
        return Ok(());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = key;
        Err(SuramakError::Keychain(
            "keychain not available on Linux; set SURAMAK_OPENAI_API_KEY instead".to_string(),
        ))
    }
}

/// Whether any smoke-harness variable is set (`SURAMAK_SMOKE_*`).
///
/// Smoke runs (CI + the Xvfb suites) must stay keychain-free — see
/// [`get_api_key`].
fn in_smoke_mode() -> bool {
    std::env::vars_os().any(|(name, _)| is_smoke_var(&name.to_string_lossy()))
}

/// Pure prefix check behind [`in_smoke_mode`] (tested without touching env).
fn is_smoke_var(name: &str) -> bool {
    name.starts_with("SURAMAK_SMOKE")
}

#[cfg(test)]
mod tests {
    use super::is_smoke_var;

    #[test]
    fn the_smoke_var_family_is_recognised() {
        assert!(is_smoke_var("SURAMAK_SMOKE_SECS"));
        assert!(is_smoke_var("SURAMAK_SMOKE_JOB"));
        assert!(is_smoke_var("SURAMAK_SMOKE_MODELS"));
        assert!(is_smoke_var("SURAMAK_SMOKE_QUIT_ANSWER"));
        assert!(!is_smoke_var("SURAMAK_OPENAI_API_KEY"));
        assert!(!is_smoke_var("HOME"));
    }
}
