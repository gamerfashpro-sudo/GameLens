//! Автообновление через GitHub Releases: на старте проверяем последний релиз,
//! если версия новее и есть gamelens.exe — предлагаем обновиться кнопкой.
use std::io::Read;

pub const REPO: &str = "gamerfashpro-sudo/GameLens";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Found {
    pub version: String,
    pub url: String,
}

fn parse_ver(v: &str) -> Vec<u32> {
    v.trim_start_matches('v').split('.').filter_map(|p| p.parse().ok()).collect()
}

pub fn newer(remote: &str, local: &str) -> bool {
    parse_ver(remote) > parse_ver(local)
}

pub fn check() -> Option<Found> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = ureq::get(&url).header("User-Agent", "gamelens").call().ok()?;
    let body = resp.body_mut().read_to_string().ok()?;
    let j: serde_json::Value = serde_json::from_str(&body).ok()?;
    // версия берётся из имени релиза (тег всегда "latest"), запасной вариант — tag_name
    let tag = j
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.trim().is_empty())
        .or_else(|| j.get("tag_name").and_then(|n| n.as_str()))?
        .to_string();
    if !newer(&tag, VERSION) {
        return None;
    }
    let asset = j.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(|n| n.as_str()) == Some("gamelens.exe"))?;
    Some(Found { version: tag, url: asset.get("browser_download_url")?.as_str()?.to_string() })
}

/// Скачивает новый exe рядом, подменяет текущий (запущенный exe в Windows можно переименовать) и перезапускает.
pub fn apply(f: &Found) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = exe.with_extension("new");
    let old = exe.with_extension("old");
    let mut resp = ureq::get(&f.url).header("User-Agent", "gamelens").call().map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    resp.body_mut().as_reader().read_to_end(&mut data).map_err(|e| e.to_string())?;
    if data.len() < 100_000 {
        return Err("скачанный файл слишком маленький, обновление отменено".into());
    }
    std::fs::write(&new, &data).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe); // откат
        return Err(e.to_string());
    }
    std::process::Command::new(&exe).arg("--restarted").spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_compare() {
        assert!(newer("v0.2.0", "0.1.9"));
        assert!(newer("0.10.0", "0.9.0"));
        assert!(!newer("v0.1.0", "0.1.0"));
    }
}
