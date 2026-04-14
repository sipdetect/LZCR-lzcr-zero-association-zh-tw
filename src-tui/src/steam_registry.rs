use crate::error::AppError;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
use winreg::enums::*;
#[cfg(target_os = "windows")]
use winreg::RegKey;

/// Limbus Company 的 Steam App ID
const LIMBUS_COMPANY_APP_ID: &str = "1973530";

#[cfg(target_os = "windows")]
pub fn find_steam_path() -> Result<PathBuf, AppError> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    // Try reading Steam installation path from registry
    let steam_key = hklm
        .open_subkey("SOFTWARE\\WOW6432Node\\Valve\\Steam")
        .or_else(|_| hklm.open_subkey("SOFTWARE\\Valve\\Steam"))
        .map_err(|e| AppError::Other(format!("Could not find Steam registry key: {}", e)))?;

    let install_path: String = steam_key
        .get_value("InstallPath")
        .map_err(|e| AppError::Other(format!("Could not read Steam installation path: {}", e)))?;

    Ok(PathBuf::from(install_path))
}

#[cfg(target_os = "windows")]
pub fn find_limbus_company_path() -> Result<PathBuf, AppError> {
    let steam_path = find_steam_path()?;

    // Check for game in Steam directory
    let steamapps_path = steam_path.join("steamapps");
    let limbus_path = steamapps_path.join("common").join("Limbus Company");

    if limbus_path.exists() {
        return Ok(limbus_path);
    }

    // If not found at default location, try reading libraryfolders.vdf to find other install locations
    let library_folders_path = steamapps_path.join("libraryfolders.vdf");
    if library_folders_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&library_folders_path) {
            // Parse VDF format to find all Steam Library paths
            let library_paths = parse_library_paths(&content);

            for lib_path in library_paths {
                let potential_path = lib_path
                    .join("steamapps")
                    .join("common")
                    .join("Limbus Company");
                if potential_path.exists() {
                    return Ok(potential_path);
                }

                // Also check SteamApps directory (case variant)
                let potential_path_alt = lib_path
                    .join("SteamApps")
                    .join("common")
                    .join("Limbus Company");
                if potential_path_alt.exists() {
                    return Ok(potential_path_alt);
                }
            }
        }
    }

    // Try finding through Steam registry game entry
    if let Ok(game_path) = find_game_by_steam_registry() {
        return Ok(game_path);
    }

    // Finally try common installation paths
    let common_paths = vec![
        PathBuf::from("C:\\Program Files (x86)\\Steam\\steamapps\\common\\Limbus Company"),
        PathBuf::from("C:\\Program Files\\Steam\\steamapps\\common\\Limbus Company"),
        PathBuf::from("D:\\Steam\\steamapps\\common\\Limbus Company"),
        PathBuf::from("E:\\Steam\\steamapps\\common\\Limbus Company"),
    ];

    for path in common_paths {
        if path.exists() {
            return Ok(path);
        }
    }

    Err(AppError::Other(
        "Could not find Limbus Company game directory".to_string(),
    ))
}

#[cfg(target_os = "windows")]
fn find_game_by_steam_registry() -> Result<PathBuf, AppError> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    // Try finding from Steam game registry entry
    let uninstall_path = format!(
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Steam App {}",
        LIMBUS_COMPANY_APP_ID
    );

    if let Ok(game_key) = hklm.open_subkey(&uninstall_path) {
        if let Ok(install_location) = game_key.get_value::<String, _>("InstallLocation") {
            let game_path = PathBuf::from(install_location);
            if game_path.exists() {
                return Ok(game_path);
            }
        }
    }

    // Try WOW6432Node path
    let uninstall_path_wow = format!(
        "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Steam App {}",
        LIMBUS_COMPANY_APP_ID
    );

    if let Ok(game_key) = hklm.open_subkey(&uninstall_path_wow) {
        if let Ok(install_location) = game_key.get_value::<String, _>("InstallLocation") {
            let game_path = PathBuf::from(install_location);
            if game_path.exists() {
                return Ok(game_path);
            }
        }
    }

    Err(AppError::Other(
        "Game installation path not found in registry".to_string(),
    ))
}

#[cfg(target_os = "windows")]
fn parse_library_paths(vdf_content: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut inside_library_folders = false;
    let mut brace_count = 0;

    for line in vdf_content.lines() {
        let trimmed = line.trim();

        // Check if entering libraryfolders block
        if trimmed.starts_with("\"libraryfolders\"") {
            inside_library_folders = true;
            continue;
        }

        if inside_library_folders {
            // Count braces to track level
            brace_count += trimmed.chars().filter(|&c| c == '{').count() as i32;
            brace_count -= trimmed.chars().filter(|&c| c == '}').count() as i32;

            // If path field is found
            if trimmed.starts_with("\"path\"") {
                if let Some(path_str) = extract_quoted_value(trimmed) {
                    // Handle double backslashes in Windows paths
                    let normalized_path = path_str.replace("\\\\", "\\");
                    let path = PathBuf::from(normalized_path);
                    if path.exists() {
                        paths.push(path);
                    }
                }
            }

            // If brace count returns to zero and we are in libraryfolders block, end parsing
            if brace_count <= 0 && inside_library_folders {
                break;
            }
        }
    }

    paths
}

#[cfg(target_os = "windows")]
fn extract_quoted_value(line: &str) -> Option<String> {
    // Extract path from lines like: "path"    "C:\\Games\\Steam"
    let parts: Vec<&str> = line.split('"').collect();
    if parts.len() >= 4 {
        return Some(parts[3].to_string());
    }
    None
}

#[cfg(not(target_os = "windows"))]
pub fn find_steam_path() -> Result<PathBuf, AppError> {
    // Try common Steam paths for Linux/macOS
    let home_dir = std::env::var("HOME")
        .map_err(|_| AppError::Other("Could not get HOME directory".to_string()))?;

    #[cfg(target_os = "linux")]
    let steam_paths = vec![
        PathBuf::from(format!("{}/.steam/steam", home_dir)),
        PathBuf::from(format!("{}/.local/share/Steam", home_dir)),
        PathBuf::from("/usr/share/steam"),
    ];

    #[cfg(target_os = "macos")]
    let steam_paths = vec![PathBuf::from(format!(
        "{}/Library/Application Support/Steam",
        home_dir
    ))];

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let steam_paths: Vec<PathBuf> = vec![];

    for path in steam_paths {
        if path.exists() {
            return Ok(path);
        }
    }

    Err(AppError::Other(
        "Could not find Steam installation directory".to_string(),
    ))
}

#[cfg(not(target_os = "windows"))]
pub fn find_limbus_company_path() -> Result<PathBuf, AppError> {
    let steam_path = find_steam_path()?;
    let limbus_path = steam_path
        .join("steamapps")
        .join("common")
        .join("Limbus Company");

    if limbus_path.exists() {
        return Ok(limbus_path);
    }

    // Try parsing libraryfolders.vdf
    let library_folders_path = steam_path.join("steamapps").join("libraryfolders.vdf");
    if library_folders_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&library_folders_path) {
            let library_paths = parse_library_paths(&content);

            for lib_path in library_paths {
                let potential_path = lib_path
                    .join("steamapps")
                    .join("common")
                    .join("Limbus Company");
                if potential_path.exists() {
                    return Ok(potential_path);
                }
            }
        }
    }

    Err(AppError::Other(
        "Could not find Limbus Company game directory".to_string(),
    ))
}

pub fn get_lang_folder_path() -> Result<PathBuf, AppError> {
    let game_path = find_limbus_company_path()?;
    let lang_path = game_path.join("LimbusCompany_Data").join("Lang");

    // Ensure Lang folder exists
    if !lang_path.exists() {
        std::fs::create_dir_all(&lang_path)
            .map_err(|e| AppError::Other(format!("Failed to create Lang folder: {}", e)))?;
    }

    Ok(lang_path)
}

/* pub fn get_config_path() -> Result<PathBuf, AppError> {
    let lang_path = get_lang_folder_path()?;
    Ok(lang_path.join("config.json"))
} */
