use reqwest::blocking::Client;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[test]
fn test_full_download_and_extract() {
    println!("\n🧪 完整工作流测试");
    println!("================================\n");

    // 第 1 步：获取 Release 信息
    println!("第 1 步：获取最新 Release 信息");
    let release = get_latest_release().expect("获取 Release 失败");
    println!("  ✓ Release Tag: {}", release.tag_name);

    // 第 2 步：获取 ZIP 下载 URL
    println!("\n第 2 步：获取 ZIP 下载 URL");
    let zip_url = get_zip_url(&release).expect("获取 ZIP URL 失败");
    println!("  ✓ ZIP URL: {}", zip_url);

    // 第 3 步：下载 ZIP 文件
    println!("\n第 3 步：下载 ZIP 文件");
    let temp_dir = PathBuf::from("./target/test_data");
    let zip_path = temp_dir.join("test.zip");

    fs::create_dir_all(&temp_dir).expect("创建临时目录失败");
    download_zip(&zip_url, &zip_path).expect("下载 ZIP 失败");
    println!(
        "  ✓ 下载完成: {} bytes",
        fs::metadata(&zip_path).unwrap().len()
    );

    // 第 4 步：检测 ZIP 根目录
    println!("\n第 4 步：检测 ZIP 根目录");
    let root_dir = detect_zip_root(&zip_path).expect("检测根目录失败");
    println!("  ✓ 根目录: {}", root_dir);

    // 第 5 步：提取文件
    println!("\n第 5 步：提取文件");
    let extract_dir = temp_dir.join("extracted");
    fs::create_dir_all(&extract_dir).expect("创建提取目录失败");
    extract_zip(&zip_path, &extract_dir, &root_dir).expect("提取文件失败");

    let entries: Vec<_> = fs::read_dir(&extract_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    println!("  ✓ 提取成功: {} 个文件/文件夹", entries.len());

    // 清理
    println!("\n第 6 步：清理临时文件");
    fs::remove_dir_all(&temp_dir).ok();
    println!("  ✓ 清理完成");

    println!("\n✅ 完整工作流测试通过！");
}

fn get_latest_release() -> Result<GitHubRelease, Box<dyn std::error::Error>> {
    let client = Client::new();
    let url =
        "https://api.github.com/repos/LocalizeLimbusCompany/LocalizeLimbusCompany/releases/latest";

    let response = client
        .get(url)
        .header("User-Agent", "LZCR-Test/1.0")
        .send()?;

    if !response.status().is_success() {
        return Err(format!("API 返回错误: {}", response.status()).into());
    }

    let release = response.json::<GitHubRelease>()?;
    Ok(release)
}

fn get_zip_url(release: &GitHubRelease) -> Result<String, Box<dyn std::error::Error>> {
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
        .ok_or("找不到 ZIP 文件")?;

    Ok(zip_asset.browser_download_url.clone())
}

fn download_zip(url: &str, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new();
    let response = client
        .get(url)
        .header("User-Agent", "LZCR-Test/1.0")
        .send()?;

    if !response.status().is_success() {
        return Err(format!("下载失败: {}", response.status()).into());
    }

    let bytes = response.bytes()?;
    fs::write(path, bytes)?;

    Ok(())
}

fn detect_zip_root(zip_path: &PathBuf) -> Result<String, Box<dyn std::error::Error>> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    if archive.len() == 0 {
        return Err("ZIP 文件为空".into());
    }

    // 获取第一个文件的路径
    let first_file = archive.by_index(0)?;
    let file_path = first_file.name();

    // 检测根目录
    if let Some(pos) = file_path.find('/') {
        let root = file_path[..pos].to_string();
        if !root.is_empty() && !root.starts_with('.') {
            return Ok(root);
        }
    }

    Ok(".".to_string())
}

fn extract_zip(
    zip_path: &PathBuf,
    extract_to: &PathBuf,
    skip_root: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let file_path = file.name();

        // 跳过根目录
        let path_to_extract = if file_path.starts_with(skip_root) {
            let remaining = &file_path[skip_root.len()..];
            if remaining.starts_with('/') {
                &remaining[1..]
            } else {
                remaining
            }
        } else {
            file_path
        };

        if path_to_extract.is_empty() {
            continue;
        }

        let target_path = extract_to.join(path_to_extract);

        if file.is_dir() {
            fs::create_dir_all(&target_path)?;
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }

            let mut target_file = fs::File::create(&target_path)?;
            std::io::copy(&mut file, &mut target_file)?;
        }
    }

    Ok(())
}
