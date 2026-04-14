use crate::error::AppError;
use crate::steam_registry;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const APP_INFO_FILE: &str = "lzcr-info.json";
const LEGACY_APP_INFO_FILE: &str = "llc-info.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub repo_owner: String,
    pub repo_name: String,
    pub source_folder: String,
    pub output_base: String,
    pub last_commit_hash: Option<String>,
    #[serde(default)]
    pub last_release_tag: Option<String>,
    #[serde(default)]
    pub last_release_date: Option<String>,
    #[serde(default)]
    pub last_voice_update_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationConfig {
    pub lang: String,
    #[serde(rename = "titleFont")]
    pub title_font: String,
    #[serde(rename = "contextFont")]
    pub context_font: String,
    #[serde(rename = "samplingPointSize")]
    pub sampling_point_size: i32,
    pub padding: i32,
}

impl Default for Config {
    fn default() -> Self {
        let output_base = steam_registry::get_lang_folder_path()
            .map(|path| path.join("LLC_zh-Hant").to_string_lossy().to_string())
            .unwrap_or_else(|_| "LLC_zh-Hant".to_string());

        Config {
            repo_owner: "LocalizeLimbusCompany".to_string(),
            repo_name: "LocalizeLimbusCompany".to_string(),
            source_folder: "Lang/LLC_zh-CN".to_string(), // 修改為完整的相對路徑
            output_base,
            last_commit_hash: None,
            last_release_tag: None,
            last_release_date: None,
            last_voice_update_date: None,
        }
    }
}

impl Default for TranslationConfig {
    fn default() -> Self {
        TranslationConfig {
            lang: "LLC_zh-Hant".to_string(),
            title_font: String::new(),
            context_font: String::new(),
            sampling_point_size: 78,
            padding: 5,
        }
    }
}

pub fn load_config() -> Result<Config, AppError> {
    let config_path = get_lzcr_info_path()?;

    if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        let mut config: Config = serde_json::from_str(&content)?;

        if !Path::new(&config.output_base).is_absolute() {
            if let Ok(lang_path) = steam_registry::get_lang_folder_path() {
                config.output_base = lang_path.join("LLC_zh-Hant").to_string_lossy().to_string();
            }
        }

        return Ok(config);
    }

    let legacy_config_path = get_legacy_llc_info_path()?;
    if legacy_config_path.exists() {
        let content = fs::read_to_string(&legacy_config_path)?;
        let mut config: Config = serde_json::from_str(&content)?;

        if !Path::new(&config.output_base).is_absolute() {
            if let Ok(lang_path) = steam_registry::get_lang_folder_path() {
                config.output_base = lang_path.join("LLC_zh-Hant").to_string_lossy().to_string();
            }
        }

        // Migrate legacy file name to the new project naming.
        save_config(&config)?;
        return Ok(config);
    }

    let config = Config::default();
    save_config(&config)?;
    Ok(config)
}

pub fn save_config(config: &Config) -> Result<(), AppError> {
    let content = serde_json::to_string_pretty(config)?;
    let config_path = get_lzcr_info_path()?;

    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    fs::write(config_path, content)?;
    Ok(())
}

/* pub fn load_translation_config() -> Result<TranslationConfig, AppError> {
    let translation_config_path = get_translation_config_path()?;

    if translation_config_path.exists() {
        let content = fs::read_to_string(&translation_config_path)?;
        let config: TranslationConfig = serde_json::from_str(&content)?;
        Ok(config)
    } else {
        let config = TranslationConfig::default();
        save_translation_config(&config)?;
        Ok(config)
    }
} */

pub fn save_translation_config(config: &TranslationConfig) -> Result<(), AppError> {
    let content = serde_json::to_string_pretty(config)?;
    let translation_config_path = get_translation_config_path()?;

    if let Some(parent) = translation_config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    fs::write(translation_config_path, content)?;
    Ok(())
}

pub fn get_lzcr_info_path() -> Result<std::path::PathBuf, AppError> {
    if let Ok(lang_path) = steam_registry::get_lang_folder_path() {
        Ok(lang_path.join(APP_INFO_FILE))
    } else {
        Ok(std::path::PathBuf::from(APP_INFO_FILE))
    }
}

fn get_legacy_llc_info_path() -> Result<std::path::PathBuf, AppError> {
    if let Ok(lang_path) = steam_registry::get_lang_folder_path() {
        Ok(lang_path.join(LEGACY_APP_INFO_FILE))
    } else {
        Ok(std::path::PathBuf::from(LEGACY_APP_INFO_FILE))
    }
}

pub fn get_translation_config_path() -> Result<std::path::PathBuf, AppError> {
    if let Ok(lang_path) = steam_registry::get_lang_folder_path() {
        Ok(lang_path.join("config.json"))
    } else {
        Ok(std::path::PathBuf::from("config.json"))
    }
}

pub fn ensure_translation_config() -> Result<(), AppError> {
    let translation_config_path = get_translation_config_path()?;

    if !translation_config_path.exists() {
        let config = TranslationConfig::default();
        save_translation_config(&config)?;
    }

    Ok(())
}

pub fn update_version_info(release_tag: &str, release_date: Option<&str>) -> Result<(), AppError> {
    let mut config = load_config()?;
    config.last_commit_hash = Some(release_tag.to_string());
    config.last_release_tag = Some(release_tag.to_string());
    config.last_release_date = release_date.map(|d| d.to_string());
    save_config(&config)?;
    Ok(())
}

pub fn update_voice_update_date(voice_date: Option<&str>) -> Result<(), AppError> {
    let mut config = load_config()?;
    config.last_voice_update_date = voice_date.map(|d| d.to_string());
    save_config(&config)?;
    Ok(())
}

pub fn should_update(current_release_tag: &str) -> Result<bool, AppError> {
    let config = load_config()?;

    match config
        .last_release_tag
        .as_ref()
        .or(config.last_commit_hash.as_ref())
    {
        Some(last_tag) => Ok(last_tag != current_release_tag),
        None => Ok(true),
    }
}
