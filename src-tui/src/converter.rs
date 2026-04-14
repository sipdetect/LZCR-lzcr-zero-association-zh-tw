use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use include_dir::{include_dir, Dir};
use reqwest::blocking::Client;
use serde::Deserialize;
use simplecc::dicts::S2TWP;
use simplecc::Dict;
use zip::ZipArchive;

use crate::config::Config;
use crate::error::AppError;
use crate::steam_registry;

static FONT_DIR: Dir<'_> = include_dir!("Font");

const VOICE_REPO_OWNER: &str = "sipdetect";
const VOICE_REPO_NAME: &str = "LimbusDialogueBoxes_ZH";
const VOICE_REPO_CONTENTS_API: &str =
    "https://api.github.com/repos/sipdetect/LimbusDialogueBoxes_ZH/contents";
const VOICE_REPO_COMMITS_API: &str =
    "https://api.github.com/repos/sipdetect/LimbusDialogueBoxes_ZH/commits?per_page=1";

macro_rules! clog {
    ($this:expr, $($arg:tt)*) => {
        if $this.progress_callback.is_none() {
            println!($($arg)*);
        }
    };
}
#[derive(Debug)]
struct FileInfo {
    relative_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    published_at: Option<String>,
    created_at: Option<String>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct RepoContentItem {
    name: String,
    download_url: Option<String>,
    #[serde(rename = "type")]
    item_type: String,
}

#[derive(Debug, Deserialize)]
struct GitHubCommitInfo {
    commit: GitHubCommitMeta,
}

#[derive(Debug, Deserialize)]
struct GitHubCommitMeta {
    committer: GitHubCommitter,
}

#[derive(Debug, Deserialize)]
struct GitHubCommitter {
    date: String,
}

#[derive(Debug)]
struct VoiceFile {
    name: String,
    content: Vec<u8>,
}

pub type ProgressCallback =
    Box<dyn Fn(f64, String, Option<String>, Option<usize>, Option<usize>) + Send>;

pub struct Converter {
    config: Config,
    client: Client,
    dict: Dict,
    progress_callback: Option<ProgressCallback>,
    cancel_flag: Option<Arc<AtomicBool>>,
}

impl Converter {
    pub fn new() -> Result<Self, AppError> {
        let config = crate::config::load_config()?;
        let client = Client::builder()
            .user_agent("LZCR-TUI/2.0")
            .timeout(std::time::Duration::from_secs(45))
            .build()
            .map_err(|e| AppError::Other(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            config,
            client,
            dict: S2TWP.clone(),
            progress_callback: None,
            cancel_flag: None,
        })
    }

    pub fn new_with_callback_and_cancel(
        callback: ProgressCallback,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<Self, AppError> {
        let mut converter = Self::new()?;
        converter.progress_callback = Some(callback);
        converter.cancel_flag = Some(cancel_flag);
        Ok(converter)
    }

    fn report_progress(
        &self,
        progress: f64,
        message: String,
        current_file: Option<String>,
        total_files: Option<usize>,
        processed_files: Option<usize>,
    ) {
        if let Some(ref callback) = self.progress_callback {
            callback(
                progress,
                message,
                current_file,
                total_files,
                processed_files,
            );
        }
    }

    fn check_cancelled(&self) -> Result<(), AppError> {
        if let Some(cancel_flag) = &self.cancel_flag {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(AppError::Other("Conversion cancelled by user".to_string()));
            }
        }
        Ok(())
    }

    fn get_latest_commit_hash(&self) -> Result<(String, Option<String>), AppError> {
        self.check_cancelled()?;
        let api_url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.config.repo_owner, self.config.repo_name
        );
        clog!(self, "[INFO] Connecting to GitHub API: {api_url}");
        self.report_progress(
            5.0,
            "Connecting to GitHub API...".to_string(),
            None,
            None,
            None,
        );

        let response = self
            .client
            .get(&api_url)
            .header("User-Agent", "LZCR-TUI/2.0")
            .send()
            .map_err(AppError::Network)?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .unwrap_or_else(|_| "Unable to read error message".to_string());
            return Err(AppError::Other(format!(
                "GitHub API returned error status {status}: {error_text}"
            )));
        }

        let release: GitHubRelease = response
            .json()
            .map_err(|e| AppError::Other(format!("Failed to parse Release info: {e}")))?;
        let release_date = normalize_release_date(
            release
                .published_at
                .as_deref()
                .or(release.created_at.as_deref()),
        );

        clog!(
            self,
            "[INFO] Found latest Release version: {}",
            release.tag_name
        );
        if let Some(date) = &release_date {
            clog!(self, "[INFO] Latest Release date: {date}");
        }
        self.report_progress(
            10.0,
            format!("Found latest version: {}", release.tag_name),
            None,
            None,
            None,
        );
        Ok((release.tag_name, release_date))
    }

    fn get_latest_release(&self) -> Result<GitHubRelease, AppError> {
        self.check_cancelled()?;
        let api_url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.config.repo_owner, self.config.repo_name
        );

        let response = self
            .client
            .get(&api_url)
            .header("User-Agent", "LZCR-TUI/2.0")
            .send()
            .map_err(AppError::Network)?;

        if !response.status().is_success() {
            return Err(AppError::Other(format!(
                "Failed to fetch Release info: {}",
                response.status()
            )));
        }

        response
            .json()
            .map_err(|e| AppError::Other(format!("Failed to parse Release info: {e}")))
    }

    fn get_latest_release_zip_url(&self) -> Result<String, AppError> {
        let release = self.get_latest_release()?;

        let zip_asset = release
            .assets
            .iter()
            .find(|asset| asset.name.ends_with(".zip") && !asset.name.contains("Source"))
            .or_else(|| {
                release
                    .assets
                    .iter()
                    .find(|asset| asset.name.ends_with(".zip"))
            })
            .ok_or_else(|| {
                AppError::Other(format!("No ZIP file found in release {}", release.tag_name))
            })?;

        clog!(
            self,
            "[INFO] Found ZIP download URL: {}",
            zip_asset.browser_download_url
        );
        Ok(zip_asset.browser_download_url.clone())
    }

    fn download_zip(&self) -> Result<Vec<u8>, AppError> {
        self.check_cancelled()?;
        clog!(self, "[INFO] Downloading release data from GitHub...");
        let download_url = self.get_latest_release_zip_url()?;

        self.report_progress(
            15.0,
            "Downloading update data...".to_string(),
            None,
            None,
            None,
        );

        let mut response = self
            .client
            .get(&download_url)
            .header("User-Agent", "LZCR-TUI/2.0")
            .send()
            .map_err(AppError::Network)?;

        let status = response.status();
        if !status.is_success() {
            return Err(AppError::Other(format!(
                "Download failed with HTTP status: {status}"
            )));
        }

        let total_size = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|ct_len| ct_len.to_str().ok())
            .and_then(|ct_len| ct_len.parse::<u64>().ok())
            .unwrap_or(0);

        let mut buffer = Vec::new();
        let mut downloaded = 0u64;
        let mut chunk = vec![0; 8192];

        loop {
            self.check_cancelled()?;
            let bytes_read = response.read(&mut chunk).map_err(AppError::Io)?;
            if bytes_read == 0 {
                break;
            }

            buffer.extend_from_slice(&chunk[..bytes_read]);
            downloaded += bytes_read as u64;

            let download_progress = if total_size > 0 {
                15.0 + (downloaded as f64 / total_size as f64) * 25.0
            } else {
                15.0 + (downloaded as f64 / 10_000_000.0).min(1.0) * 25.0
            };

            self.report_progress(
                download_progress,
                format!(
                    "Downloading... {} / {}",
                    format_bytes(downloaded),
                    format_bytes(total_size)
                ),
                None,
                None,
                None,
            );
        }

        self.report_progress(40.0, "Download completed".to_string(), None, None, None);
        Ok(buffer)
    }

    fn extract_files(&self, zip_data: Vec<u8>) -> Result<Vec<FileInfo>, AppError> {
        self.check_cancelled()?;
        clog!(self, "[INFO] Extracting files from archive...");
        self.report_progress(45.0, "Extracting files...".to_string(), None, None, None);

        let cursor = std::io::Cursor::new(zip_data);
        let mut archive = ZipArchive::new(cursor)?;
        let mut files = Vec::new();

        if Path::new("temp").exists() {
            fs::remove_dir_all("temp")?;
        }
        fs::create_dir_all("temp")?;

        let mut root_dir = String::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            let file_path = file.name();
            if let Some(first_slash) = file_path.find('/') {
                root_dir = file_path[..first_slash].to_string();
                clog!(self, "[INFO] Detected root directory in ZIP: {root_dir}");
                break;
            }
        }

        let target_folder = if root_dir.is_empty() {
            self.config.source_folder.clone()
        } else {
            format!("{root_dir}/{}", self.config.source_folder)
        };

        clog!(self, "[INFO] Looking for source folder: {target_folder}");

        let total_files = archive.len();
        let mut processed = 0usize;

        for i in 0..archive.len() {
            if i % 200 == 0 {
                self.check_cancelled()?;
            }

            let mut file = match archive.by_index(i) {
                Ok(f) => f,
                Err(e) => {
                    clog!(self, "[WARN] Failed to access file at index {i}: {e}");
                    continue;
                }
            };

            let file_path_str = file.name().to_string();
            if !file_path_str.starts_with(&target_folder) || !file_path_str.ends_with(".json") {
                continue;
            }

            let relative_str = if file_path_str.starts_with(&format!("{target_folder}/")) {
                &file_path_str[target_folder.len() + 1..]
            } else {
                &file_path_str[target_folder.len()..]
            };

            let relative_path = PathBuf::from(relative_str);
            let mut content = Vec::new();
            if let Err(e) = file.read_to_end(&mut content) {
                clog!(
                    self,
                    "[ERROR] Failed to read file content for {file_path_str}: {e}"
                );
                continue;
            }

            let temp_path = Path::new("temp").join(&relative_path);
            if let Some(parent) = temp_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    clog!(
                        self,
                        "[ERROR] Failed to create parent directory for {}: {}",
                        temp_path.display(),
                        e
                    );
                    continue;
                }
            }

            if let Err(e) = fs::write(&temp_path, &content) {
                clog!(
                    self,
                    "[ERROR] Failed to write file {}: {}",
                    temp_path.display(),
                    e
                );
                continue;
            }

            files.push(FileInfo { relative_path });
            processed += 1;

            if processed % 100 == 0 {
                let extract_progress = 45.0 + (processed as f64 / total_files as f64) * 10.0;
                self.report_progress(
                    extract_progress,
                    format!("Extracting... {processed} files"),
                    None,
                    None,
                    None,
                );
            }
        }

        clog!(
            self,
            "[INFO] Finished extraction. Found {} JSON files",
            files.len()
        );
        self.report_progress(
            55.0,
            format!("Found {} files to convert", files.len()),
            None,
            None,
            None,
        );
        Ok(files)
    }

    fn convert_file(&self, input_path: &Path, output_path: &Path) -> Result<(), AppError> {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(AppError::Io)?;
        }

        let bytes = fs::read(input_path).map_err(AppError::Io)?;
        let content = String::from_utf8(bytes).unwrap_or_else(|e| {
            clog!(
                self,
                "[WARN] File {} is not valid UTF-8, using lossy conversion",
                input_path.display()
            );
            String::from_utf8_lossy(e.as_bytes()).into_owned()
        });

        let converted = self.dict.replace_all(&content);
        fs::write(output_path, converted.as_bytes()).map_err(AppError::Io)?;
        Ok(())
    }

    fn process_files(&self, files: Vec<FileInfo>) -> Result<(), AppError> {
        clog!(self, "[INFO] Starting file conversion...");
        let total_files = files.len();
        self.report_progress(
            60.0,
            "Starting file conversion...".to_string(),
            None,
            Some(total_files),
            Some(0),
        );

        let mut conversion_errors = 0usize;
        for (index, file_info) in files.iter().enumerate() {
            self.check_cancelled()?;

            let source_file = Path::new("temp").join(&file_info.relative_path);
            let output_file = Path::new(&self.config.output_base).join(&file_info.relative_path);

            let file_name = file_info
                .relative_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let convert_progress = if total_files > 0 {
                60.0 + (index as f64 / total_files as f64) * 30.0
            } else {
                90.0
            };
            self.report_progress(
                convert_progress,
                format!("Converting... {} / {} files", index + 1, total_files),
                Some(file_name),
                Some(total_files),
                Some(index + 1),
            );

            if let Err(e) = self.convert_file(&source_file, &output_file) {
                conversion_errors += 1;
                clog!(
                    self,
                    "[WARN] Failed to convert file {}: {}",
                    file_info.relative_path.display(),
                    e
                );
                if conversion_errors > 10 {
                    clog!(self, "[ERROR] Too many conversion errors, stopping");
                    return Err(e);
                }
            }
        }

        clog!(
            self,
            "[INFO] Conversion completed with {conversion_errors} errors"
        );
        self.report_progress(
            90.0,
            "File conversion complete!".to_string(),
            None,
            Some(total_files),
            Some(total_files),
        );
        Ok(())
    }

    fn write_font_folder(&self) -> Result<(), AppError> {
        self.check_cancelled()?;
        self.report_progress(92.0, "Copying font files...".to_string(), None, None, None);

        let out_font_dir = Path::new(&self.config.output_base).join("Font");
        if out_font_dir.exists() {
            fs::remove_dir_all(&out_font_dir)?;
        }

        self.write_dir(&FONT_DIR, &out_font_dir)?;
        clog!(
            self,
            "[INFO] Font folder exported to {}",
            out_font_dir.display()
        );
        self.report_progress(
            95.0,
            "Font file copy complete!".to_string(),
            None,
            None,
            None,
        );
        Ok(())
    }

    fn write_dir(&self, dir: &Dir, path: &Path) -> Result<(), AppError> {
        fs::create_dir_all(path)?;

        for file in dir.files() {
            let file_path = path.join(file.path().file_name().unwrap());
            fs::write(&file_path, file.contents())?;
        }

        for subdir in dir.dirs() {
            let sub_path = path.join(subdir.path().file_name().unwrap());
            self.write_dir(subdir, &sub_path)?;
        }

        Ok(())
    }

    fn show_installation_info(&self) -> Result<(), AppError> {
        self.check_cancelled()?;
        clog!(self, "[INFO] Looking for Limbus Company game directory...");
        self.report_progress(
            2.0,
            "Looking for game directory...".to_string(),
            None,
            None,
            None,
        );

        match steam_registry::find_limbus_company_path() {
            Ok(game_path) => {
                clog!(self, "[OK] Found Limbus Company game directory:");
                clog!(self, "   {}", game_path.display());

                let lang_path = game_path.join("LimbusCompany_Data").join("Lang");
                clog!(self, "[INFO] Lang folder location:");
                clog!(self, "   {}", lang_path.display());

                let llc_path = lang_path.join("LLC_zh-Hant");
                clog!(
                    self,
                    "[INFO] Traditional Chinese package will be installed to:"
                );
                clog!(self, "   {}", llc_path.display());

                self.report_progress(3.0, "Game directory found".to_string(), None, None, None);
            }
            Err(e) => {
                clog!(
                    self,
                    "[WARN] Could not find Limbus Company game directory: {e}"
                );
                clog!(
                    self,
                    "   Using default output directory: {}",
                    self.config.output_base
                );
                self.report_progress(
                    3.0,
                    "Game directory not found, using default location".to_string(),
                    None,
                    None,
                    None,
                );
            }
        }

        Ok(())
    }

    pub fn run(&mut self) -> Result<(), AppError> {
        clog!(self, "[INFO] ===== CONVERTER RUN START =====");
        self.show_installation_info()?;
        self.check_cancelled()?;

        let (latest_release_tag, latest_release_date) = self.get_latest_commit_hash()?;
        clog!(self, "[INFO] Latest Release version: {latest_release_tag}");
        if let Some(date) = &latest_release_date {
            clog!(self, "[INFO] Using release date: {date}");
        }

        if crate::config::should_update(&latest_release_tag)? {
            clog!(self, "[INFO] Downloading ZIP from GitHub...");
            let zip_data = self.download_zip()?;
            clog!(self, "[OK] ZIP downloaded successfully");

            clog!(self, "[INFO] Extracting files...");
            let files = self.extract_files(zip_data)?;
            clog!(self, "[OK] Extraction completed");

            clog!(self, "[INFO] Converting files...");
            self.process_files(files)?;
            clog!(self, "[OK] File conversion completed");

            clog!(self, "[INFO] Updating version info...");
            crate::config::update_version_info(&latest_release_tag, latest_release_date.as_deref())?;
            clog!(self, "[OK] Version info updated");
        } else {
            clog!(self, "[OK] Localization package already up to date");
            crate::config::update_version_info(&latest_release_tag, latest_release_date.as_deref())?;
            self.report_progress(
                90.0,
                "Localization package already up to date".to_string(),
                None,
                None,
                None,
            );
        }

        self.check_cancelled()?;
        self.write_font_folder()?;

        self.report_progress(
            96.0,
            "Creating config file...".to_string(),
            None,
            None,
            None,
        );
        self.create_game_config()?;

        self.report_progress(
            97.0,
            "Updating config file...".to_string(),
            None,
            None,
            None,
        );
        crate::config::ensure_translation_config()?;

        self.report_progress(
            98.0,
            "Cleaning temporary files...".to_string(),
            None,
            None,
            None,
        );
        if Path::new("temp").exists() {
            fs::remove_dir_all("temp")?;
        }

        self.report_progress(
            99.0,
            "Syncing voice files from GitHub...".to_string(),
            None,
            None,
            None,
        );
        let voice_update_date = self.copy_voice_files()?;
        crate::config::update_voice_update_date(voice_update_date.as_deref())?;

        clog!(self, "[OK] Conversion complete!");
        clog!(self, "[INFO] ===== CONVERTER RUN FINISH =====");
        self.report_progress(100.0, "Conversion complete!".to_string(), None, None, None);
        Ok(())
    }

    fn create_game_config(&self) -> Result<(), AppError> {
        self.check_cancelled()?;
        let full_output_path = Path::new(&self.config.output_base);

        if let Some(lang_dir) = full_output_path.parent() {
            let config_file_path = lang_dir.join("config.json");
            let config_content = r#"{
  "lang": "LLC_zh-Hant",
  "titleFont": "",
  "contextFont": "",
  "samplingPointSize": 78,
  "padding": 5
}"#;

            fs::write(&config_file_path, config_content).map_err(|e| {
                AppError::Other(format!(
                    "Failed to create config file {}: {}",
                    config_file_path.display(),
                    e
                ))
            })?;
            Ok(())
        } else {
            Err(AppError::Other(format!(
                "Failed to parse Lang folder path: {}",
                self.config.output_base
            )))
        }
    }

    fn download_voice_files(&self) -> Result<Vec<VoiceFile>, AppError> {
        self.check_cancelled()?;
        let response = self
            .client
            .get(VOICE_REPO_CONTENTS_API)
            .header("User-Agent", "LZCR-TUI/2.0")
            .send()
            .map_err(AppError::Network)?;

        if !response.status().is_success() {
            return Err(AppError::Other(format!(
                "Failed to fetch voice repo file list: {}",
                response.status()
            )));
        }

        let mut entries: Vec<RepoContentItem> = response
            .json()
            .map_err(|e| AppError::Other(format!("Failed to parse voice repo file list: {e}")))?;

        entries.retain(|item| item.item_type == "file" && item.name.ends_with(".json"));
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        if entries.is_empty() {
            return Err(AppError::Other(format!(
                "No JSON files found in voice repo {VOICE_REPO_OWNER}/{VOICE_REPO_NAME}"
            )));
        }

        let total_files = entries.len();
        let mut downloaded_files = Vec::with_capacity(total_files);

        for (index, entry) in entries.iter().enumerate() {
            self.check_cancelled()?;

            let download_url = entry.download_url.as_ref().ok_or_else(|| {
                AppError::Other(format!("Missing download_url for file {}", entry.name))
            })?;

            self.report_progress(
                99.1 + (index as f64 / total_files as f64) * 0.4,
                format!(
                    "Downloading voice file {} ({}/{})",
                    entry.name,
                    index + 1,
                    total_files
                ),
                Some(entry.name.clone()),
                Some(total_files),
                Some(index + 1),
            );

            let file_response = self
                .client
                .get(download_url)
                .header("User-Agent", "LZCR-TUI/2.0")
                .send()
                .map_err(AppError::Network)?;

            if !file_response.status().is_success() {
                return Err(AppError::Other(format!(
                    "Failed to download voice file {}: {}",
                    entry.name,
                    file_response.status()
                )));
            }

            let bytes = file_response.bytes().map_err(AppError::Network)?;
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| {
                AppError::Other(format!(
                    "Voice file {} is not valid JSON: {}",
                    entry.name, e
                ))
            })?;

            downloaded_files.push(VoiceFile {
                name: entry.name.clone(),
                content: bytes.to_vec(),
            });
        }

        Ok(downloaded_files)
    }

    fn get_voice_latest_update_date(&self) -> Result<Option<String>, AppError> {
        self.check_cancelled()?;
        let response = self
            .client
            .get(VOICE_REPO_COMMITS_API)
            .header("User-Agent", "LZCR-TUI/2.0")
            .send()
            .map_err(AppError::Network)?;

        if !response.status().is_success() {
            return Err(AppError::Other(format!(
                "Failed to fetch voice repo commits: {}",
                response.status()
            )));
        }

        let commits: Vec<GitHubCommitInfo> = response
            .json()
            .map_err(|e| AppError::Other(format!("Failed to parse voice repo commits: {e}")))?;

        Ok(commits
            .first()
            .and_then(|c| normalize_release_date(Some(c.commit.committer.date.as_str()))))
    }

    fn cleanup_old_voice_files(&self, target_dir: &Path) -> Result<(), AppError> {
        let old_files = [
            "BattleSpeechBubbleDlg.json",
            "BattleSpeechBubbleDlg_Cultivation.json",
            "BattleSpeechBubbleDlg_mowe.json",
        ];

        for old_file in old_files {
            let file_path = target_dir.join(old_file);
            if file_path.exists() {
                fs::remove_file(&file_path).map_err(|e| {
                    AppError::Other(format!(
                        "Failed to remove old voice file {}: {}",
                        old_file, e
                    ))
                })?;
            }
        }

        Ok(())
    }

    fn copy_voice_files(&self) -> Result<Option<String>, AppError> {
        self.check_cancelled()?;
        let llc_path = Path::new(&self.config.output_base);
        fs::create_dir_all(llc_path).map_err(|e| {
            AppError::Other(format!("Failed to create LLC_zh-Hant directory: {}", e))
        })?;

        clog!(
            self,
            "[INFO] Syncing voice files from {}/{}",
            VOICE_REPO_OWNER,
            VOICE_REPO_NAME
        );
        let voice_update_date = match self.get_voice_latest_update_date() {
            Ok(date) => date,
            Err(err) => {
                clog!(self, "[WARN] Could not resolve voice update date: {err}");
                None
            }
        };

        let voice_files = self.download_voice_files()?;
        self.cleanup_old_voice_files(llc_path)?;

        let total = voice_files.len();
        for (index, voice_file) in voice_files.iter().enumerate() {
            self.check_cancelled()?;

            let destination = llc_path.join(&voice_file.name);
            fs::write(&destination, &voice_file.content).map_err(|e| {
                AppError::Other(format!(
                    "Failed to write voice file {}: {}",
                    voice_file.name, e
                ))
            })?;

            self.report_progress(
                99.6 + (index as f64 / total as f64) * 0.3,
                format!(
                    "Installed voice file {} ({}/{})",
                    voice_file.name,
                    index + 1,
                    total
                ),
                Some(voice_file.name.clone()),
                Some(total),
                Some(index + 1),
            );
            clog!(self, "[OK] Voice file copied: {}", voice_file.name);
        }

        Ok(voice_update_date)
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut size = bytes as f64;
    let mut unit_index = 0usize;
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{size:.2} {}", UNITS[unit_index])
}

fn normalize_release_date(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim();
    if value.len() >= 10 {
        let date = &value[..10];
        if date.chars().all(|c| c.is_ascii_digit() || c == '-') {
            return Some(date.to_string());
        }
    }
    None
}
