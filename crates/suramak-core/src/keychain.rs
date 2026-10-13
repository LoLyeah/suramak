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
