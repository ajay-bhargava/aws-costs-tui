//! AWS Credentials loading from multiple sources
//!
//! Supports:
//! - Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN)
//! - AWS profiles (~/.aws/credentials and ~/.aws/config)

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use tracing::debug;

/// AWS credentials
#[derive(Debug, Clone)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: String,
}

impl Credentials {
    /// Load credentials for a given profile
    pub fn load(profile: &str, region: Option<&str>) -> Result<Self> {
        // Determine region
        let region = region
            .map(|r| r.to_string())
            .or_else(|| env::var("AWS_REGION").ok())
            .or_else(|| env::var("AWS_DEFAULT_REGION").ok())
            .or_else(|| get_profile_region(profile))
            .unwrap_or_else(|| "us-east-1".to_string());

        // 1. Try environment variables first (if default profile)
        if profile == "default" {
            if let Ok(creds) = load_from_env(&region) {
                debug!("Loaded credentials from environment variables");
                return Ok(creds);
            }
        }

        // 2. Try AWS credentials file
        if let Ok(creds) = load_from_credentials_file(profile, &region) {
            debug!("Loaded credentials from credentials file for profile '{}'", profile);
            return Ok(creds);
        }

        // 3. Try config file with direct credentials
        if let Ok(creds) = load_from_config_file(profile, &region) {
            debug!("Loaded credentials from config file for profile '{}'", profile);
            return Ok(creds);
        }

        Err(anyhow!(
            "No credentials found for profile '{}'. Run 'aws configure' or set AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY",
            profile
        ))
    }
}

/// Load credentials from environment variables
fn load_from_env(region: &str) -> Result<Credentials> {
    let access_key_id = env::var("AWS_ACCESS_KEY_ID")
        .map_err(|_| anyhow!("AWS_ACCESS_KEY_ID not set"))?;
    let secret_access_key = env::var("AWS_SECRET_ACCESS_KEY")
        .map_err(|_| anyhow!("AWS_SECRET_ACCESS_KEY not set"))?;
    let session_token = env::var("AWS_SESSION_TOKEN").ok();

    Ok(Credentials {
        access_key_id,
        secret_access_key,
        session_token,
        region: region.to_string(),
    })
}

/// Get AWS config directory
fn aws_config_dir() -> Result<PathBuf> {
    if let Ok(path) = env::var("AWS_CONFIG_FILE") {
        if let Some(parent) = PathBuf::from(path).parent() {
            return Ok(parent.to_path_buf());
        }
    }

    dirs::home_dir()
        .map(|h| h.join(".aws"))
        .ok_or_else(|| anyhow!("Could not find home directory"))
}

/// Parse an INI-style file into sections
fn parse_ini_file(content: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section = String::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        // Section header
        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].trim().to_string();
            // Handle "profile name" format in config file
            if current_section.starts_with("profile ") {
                current_section = current_section["profile ".len()..].to_string();
            }
            sections.entry(current_section.clone()).or_default();
            continue;
        }

        // Key-value pair
        if let Some((key, value)) = line.split_once('=') {
            if !current_section.is_empty() {
                sections
                    .entry(current_section.clone())
                    .or_default()
                    .insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }

    sections
}

/// Load credentials from ~/.aws/credentials
fn load_from_credentials_file(profile: &str, region: &str) -> Result<Credentials> {
    let creds_path = aws_config_dir()?.join("credentials");
    let content = fs::read_to_string(&creds_path)
        .map_err(|_| anyhow!("Could not read {:?}", creds_path))?;

    let sections = parse_ini_file(&content);

    let section = sections
        .get(profile)
        .ok_or_else(|| anyhow!("Profile '{}' not found in credentials file", profile))?;

    let access_key_id = section
        .get("aws_access_key_id")
        .ok_or_else(|| anyhow!("aws_access_key_id not found for profile '{}'", profile))?
        .clone();

    let secret_access_key = section
        .get("aws_secret_access_key")
        .ok_or_else(|| anyhow!("aws_secret_access_key not found for profile '{}'", profile))?
        .clone();

    let session_token = section.get("aws_session_token").cloned();

    Ok(Credentials {
        access_key_id,
        secret_access_key,
        session_token,
        region: region.to_string(),
    })
}

/// Load credentials from ~/.aws/config
fn load_from_config_file(profile: &str, region: &str) -> Result<Credentials> {
    let config_path = aws_config_dir()?.join("config");
    let content = fs::read_to_string(&config_path)
        .map_err(|_| anyhow!("Could not read {:?}", config_path))?;

    let sections = parse_ini_file(&content);

    let section = sections
        .get(profile)
        .ok_or_else(|| anyhow!("Profile '{}' not found in config file", profile))?;

    if let (Some(access_key), Some(secret_key)) = (
        section.get("aws_access_key_id"),
        section.get("aws_secret_access_key"),
    ) {
        return Ok(Credentials {
            access_key_id: access_key.clone(),
            secret_access_key: secret_key.clone(),
            session_token: section.get("aws_session_token").cloned(),
            region: region.to_string(),
        });
    }

    Err(anyhow!(
        "No direct credentials found in config for profile '{}'",
        profile
    ))
}

/// Get the default region for a profile
fn get_profile_region(profile: &str) -> Option<String> {
    if let Ok(config_dir) = aws_config_dir() {
        let config_path = config_dir.join("config");
        if let Ok(content) = fs::read_to_string(&config_path) {
            let sections = parse_ini_file(&content);
            if let Some(section) = sections.get(profile) {
                if let Some(region) = section.get("region") {
                    return Some(region.clone());
                }
            }
        }
    }
    None
}

/// List available AWS profiles
#[allow(dead_code)]
pub fn list_profiles() -> Vec<String> {
    let mut profiles = Vec::new();

    if let Ok(config_dir) = aws_config_dir() {
        // Read from credentials file
        if let Ok(content) = fs::read_to_string(config_dir.join("credentials")) {
            let sections = parse_ini_file(&content);
            profiles.extend(sections.keys().cloned());
        }

        // Read from config file
        if let Ok(content) = fs::read_to_string(config_dir.join("config")) {
            let sections = parse_ini_file(&content);
            for key in sections.keys() {
                if !profiles.contains(key) {
                    profiles.push(key.clone());
                }
            }
        }
    }

    profiles.sort();
    profiles
}
