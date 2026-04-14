use reqwest::blocking::Client;
use serde::Deserialize;

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
fn test_github_releases_workflow() {
    println!("🧪 测试 GitHub Releases 功能");
    println!("================================\n");

    // 测试 1: 获取最新 Release
    println!("✅ 测试 1: 获取最新 Release 信息");
    match get_latest_release() {
        Ok(release) => {
            println!("  ✓ Release Tag: {}", release.tag_name);
            println!("  ✓ Assets 数量: {}", release.assets.len());

            // 测试 2: 获取 ZIP URL
            println!("\n✅ 测试 2: 获取 ZIP 下载 URL");
            match get_zip_url(&release) {
                Ok(url) => {
                    println!("  ✓ ZIP URL: {}", url);

                    // 测试 3: 下载文件头部（验证 URL 有效）
                    println!("\n✅ 测试 3: 验证 ZIP 文件可下载");
                    match test_download(&url) {
                        Ok(size) => {
                            println!("  ✓ 文件大小: {:.2} MB", size as f64 / 1_000_000.0);
                            println!("\n✅ 所有测试通过！");
                        }
                        Err(e) => println!("  ✗ 下载失败: {}", e),
                    }
                }
                Err(e) => println!("  ✗ 获取 URL 失败: {}", e),
            }
        }
        Err(e) => println!("  ✗ 获取 Release 失败: {}", e),
    }
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

fn test_download(url: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let client = Client::new();
    let response = client
        .head(url)
        .header("User-Agent", "LZCR-Test/1.0")
        .send()?;

    if !response.status().is_success() {
        return Err(format!("下载失败: {}", response.status()).into());
    }

    let size = response
        .headers()
        .get("content-length")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    Ok(size)
}
