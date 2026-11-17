use serde::Serialize;
use serde_json::Value;
use std::{path::PathBuf, process::Command};
use tauri::{AppHandle, Manager};
use url::Url;

const MAX_URL_LENGTH: usize = 2_048;
const MAX_PLAYLIST_TRACKS: &str = "200";

#[derive(Debug, Serialize)]
pub struct EngineStatus {
    pub available: bool,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct TrackInfo {
    pub id: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub duration: Option<f64>,
    pub url: Option<String>,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MediaInfo {
    pub title: String,
    pub artist: Option<String>,
    pub duration: Option<f64>,
    pub url: Option<String>,
    pub thumbnail: Option<String>,
    pub is_playlist: bool,
    pub tracks: Vec<TrackInfo>,
}

fn find_ytdlp(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join("binaries").join("yt-dlp.exe"));
        candidates.push(resource_dir.join("yt-dlp.exe"));
    }
    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir.join("src-tauri").join("binaries").join("yt-dlp.exe"));
        candidates.push(current_dir.join("binaries").join("yt-dlp.exe"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn validate_soundcloud_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_URL_LENGTH {
        return Err("URL을 입력해 주세요. 최대 길이는 2,048자입니다.".into());
    }
    let parsed = Url::parse(trimmed).map_err(|_| "올바른 URL 형식이 아닙니다.".to_string())?;
    if parsed.scheme() != "https" {
        return Err("보안을 위해 HTTPS SoundCloud 링크만 지원합니다.".into());
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if host != "soundcloud.com" && !host.ends_with(".soundcloud.com") {
        return Err("SoundCloud 도메인의 링크만 분석할 수 있습니다.".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("사용자 정보가 포함된 URL은 허용되지 않습니다.".into());
    }
    Ok(parsed.to_string())
}

fn track_from_json(value: &Value) -> Option<TrackInfo> {
    let title = value.get("title")?.as_str()?.trim();
    if title.is_empty() {
        return None;
    }
    Some(TrackInfo {
        id: value.get("id").and_then(Value::as_str).map(str::to_owned),
        title: title.to_owned(),
        artist: value
            .get("uploader")
            .or_else(|| value.get("artist"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        duration: value.get("duration").and_then(Value::as_f64),
        url: value
            .get("webpage_url")
            .or_else(|| value.get("url"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        thumbnail: value.get("thumbnail").and_then(Value::as_str).map(str::to_owned),
    })
}

fn parse_media_info(json: &str) -> Result<MediaInfo, String> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| format!("yt-dlp 응답을 JSON으로 읽지 못했습니다: {error}"))?;
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| "SoundCloud에서 제목 정보를 찾지 못했습니다.".to_string())?
        .to_owned();
    let raw_entries = value.get("entries").and_then(Value::as_array);
    let is_playlist = raw_entries.is_some() || value.get("_type").and_then(Value::as_str) == Some("playlist");
    let tracks = match raw_entries {
        Some(entries) => entries.iter().filter_map(track_from_json).collect(),
        None => track_from_json(&value).into_iter().collect(),
    };
    Ok(MediaInfo {
        title,
        artist: value
            .get("uploader")
            .or_else(|| value.get("artist"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        duration: value.get("duration").and_then(Value::as_f64),
        url: value.get("webpage_url").and_then(Value::as_str).map(str::to_owned),
        thumbnail: value.get("thumbnail").and_then(Value::as_str).map(str::to_owned),
        is_playlist,
        tracks,
    })
}

#[tauri::command]
pub async fn engine_status(app: AppHandle) -> Result<EngineStatus, String> {
    let Some(executable) = find_ytdlp(&app) else {
        return Ok(EngineStatus {
            available: false,
            version: None,
            message: "yt-dlp.exe가 없습니다. src-tauri/binaries/yt-dlp.exe에 공식 실행 파일을 추가해 주세요.".into(),
        });
    };
    tauri::async_runtime::spawn_blocking(move || match Command::new(executable).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            Ok(EngineStatus {
                available: true,
                version: (!version.is_empty()).then_some(version),
                message: "yt-dlp를 사용할 수 있습니다.".into(),
            })
        }
        Ok(output) => Ok(EngineStatus {
            available: false,
            version: None,
            message: format!("yt-dlp 실행 확인에 실패했습니다 ({}).", output.status),
        }),
        Err(error) => Ok(EngineStatus {
            available: false,
            version: None,
            message: format!("yt-dlp를 실행할 수 없습니다: {error}"),
        }),
    })
    .await
    .map_err(|error| format!("작업 스레드를 시작하지 못했습니다: {error}"))?
}

#[tauri::command]
pub async fn inspect_soundcloud_url(app: AppHandle, url: String) -> Result<MediaInfo, String> {
    let url = validate_soundcloud_url(&url)?;
    let executable = find_ytdlp(&app).ok_or_else(|| {
        "yt-dlp.exe를 찾을 수 없습니다. 공식 실행 파일을 src-tauri/binaries/yt-dlp.exe에 배치한 뒤 다시 시도해 주세요.".to_string()
    })?;

    tauri::async_runtime::spawn_blocking(move || {
        let output = Command::new(executable)
            .args([
                "--dump-single-json",
                "--flat-playlist",
                "--no-warnings",
                "--no-call-home",
                "--skip-download",
                "--socket-timeout",
                "15",
                "--playlist-end",
                MAX_PLAYLIST_TRACKS,
                "--",
            ])
            .arg(url)
            .output()
            .map_err(|error| format!("yt-dlp 프로세스를 시작하지 못했습니다: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let details = stderr.trim();
            let details = if details.len() > 500 { &details[..500] } else { details };
            return Err(if details.is_empty() {
                format!("yt-dlp가 오류 코드 {}로 종료되었습니다.", output.status)
            } else {
                format!("SoundCloud 정보를 가져오지 못했습니다: {details}")
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_media_info(&stdout)
    })
    .await
    .map_err(|error| format!("분석 작업을 시작하지 못했습니다: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::validate_soundcloud_url;

    #[test]
    fn accepts_https_soundcloud_links() {
        assert!(validate_soundcloud_url("https://soundcloud.com/artist/track").is_ok());
        assert!(validate_soundcloud_url("https://on.soundcloud.com/example").is_ok());
    }

    #[test]
    fn rejects_non_soundcloud_or_insecure_links() {
        assert!(validate_soundcloud_url("http://soundcloud.com/artist/track").is_err());
        assert!(validate_soundcloud_url("https://example.com/track").is_err());
        assert!(validate_soundcloud_url("https://soundcloud.com.example.org/track").is_err());
    }
}
