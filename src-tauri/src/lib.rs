use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};
use tauri::{Manager, State};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub download_directory: String,
    pub audio_format: String,
    pub audio_quality: String,
    pub max_concurrent: usize,
    pub overwrite_files: bool,
    pub embed_artwork: bool,
    pub embed_metadata: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            download_directory: default_download_directory().to_string_lossy().into_owned(),
            audio_format: "original".into(),
            audio_quality: "192".into(),
            max_concurrent: 2,
            overwrite_files: false,
            embed_artwork: true,
            embed_metadata: true,
        }
    }
}

impl AppSettings {
    fn validate(&self) -> Result<(), String> {
        if self.download_directory.trim().is_empty() {
            return Err("다운로드 폴더를 입력하세요.".into());
        }
        if !matches!(self.audio_format.as_str(), "original" | "mp3") {
            return Err("지원하지 않는 오디오 형식입니다.".into());
        }
        if !matches!(self.audio_quality.as_str(), "128" | "192" | "256" | "320") {
            return Err("음질은 128, 192, 256, 320 kbps 중에서 선택하세요.".into());
        }
        if !(1..=4).contains(&self.max_concurrent) {
            return Err("동시 작업 수는 1~4개로 설정해야 합니다.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(default)]
struct Store {
    settings: AppSettings,
}

struct AppState {
    store_path: PathBuf,
    store: Mutex<Store>,
}

impl AppState {
    fn load(store_path: PathBuf, download_directory: PathBuf) -> Result<Self, String> {
        let store = match fs::read(&store_path) {
            Ok(contents) => serde_json::from_slice::<Store>(&contents)
                .map_err(|error| format!("저장된 설정을 읽을 수 없습니다: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut store = Store::default();
                store.settings.download_directory =
                    download_directory.to_string_lossy().into_owned();
                store
            }
            Err(error) => return Err(format!("앱 데이터 파일을 읽을 수 없습니다: {error}")),
        };
        store.settings.validate()?;

        let state = Self {
            store_path,
            store: Mutex::new(store),
        };
        state.persist()?;
        Ok(state)
    }

    fn persist(&self) -> Result<(), String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?;
        self.persist_store(&store)
    }

    fn persist_store(&self, store: &Store) -> Result<(), String> {
        let parent = self
            .store_path
            .parent()
            .ok_or_else(|| "앱 데이터 경로가 올바르지 않습니다.".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("앱 데이터 폴더를 만들 수 없습니다: {error}"))?;

        let temporary_path = self.store_path.with_extension("json.tmp");
        let contents = serde_json::to_vec_pretty(store)
            .map_err(|error| format!("앱 설정을 JSON으로 변환할 수 없습니다: {error}"))?;
        fs::write(&temporary_path, contents)
            .map_err(|error| format!("앱 설정을 저장할 수 없습니다: {error}"))?;
        replace_file(&temporary_path, &self.store_path)
    }
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|error| format!("기존 앱 설정을 교체할 수 없습니다: {error}"))?;
    }

    fs::rename(source, destination).map_err(|error| {
        let _ = fs::remove_file(source);
        format!("앱 설정을 저장할 수 없습니다: {error}")
    })
}

fn default_download_directory() -> PathBuf {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub available: bool,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub yt_dlp: ToolStatus,
    pub ffmpeg: ToolStatus,
}

fn locate_executable(names: &[&str]) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}

fn inspect_tool(names: &[&str]) -> ToolStatus {
    let Some(executable) = locate_executable(names) else {
        return ToolStatus {
            available: false,
            version: None,
            error: Some("실행 파일을 PATH에서 찾지 못했습니다.".into()),
        };
    };

    match Command::new(executable).arg("--version").output() {
        Ok(output) if output.status.success() => ToolStatus {
            available: true,
            version: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            error: None,
        },
        Ok(output) => ToolStatus {
            available: false,
            version: None,
            error: Some(format!(
                "버전 확인에 실패했습니다: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        },
        Err(error) => ToolStatus {
            available: false,
            version: None,
            error: Some(format!("실행 파일을 확인할 수 없습니다: {error}")),
        },
    }
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    state
        .store
        .lock()
        .map(|store| store.settings.clone())
        .map_err(|_| "앱 설정을 읽을 수 없습니다.".into())
}

#[tauri::command]
fn save_settings(settings: AppSettings, state: State<'_, AppState>) -> Result<AppSettings, String> {
    settings.validate()?;
    let mut store = state
        .store
        .lock()
        .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?;
    let previous_settings = store.settings.clone();
    store.settings = settings;
    if let Err(error) = state.persist_store(&store) {
        store.settings = previous_settings;
        return Err(error);
    }
    Ok(store.settings.clone())
}

#[tauri::command]
fn get_engine_status() -> EngineStatus {
    #[cfg(windows)]
    let (yt_dlp_names, ffmpeg_names) = (["yt-dlp.exe", "yt-dlp"], ["ffmpeg.exe", "ffmpeg"]);
    #[cfg(not(windows))]
    let (yt_dlp_names, ffmpeg_names) = (["yt-dlp", "yt-dlp"], ["ffmpeg", "ffmpeg"]);

    EngineStatus {
        yt_dlp: inspect_tool(&yt_dlp_names),
        ffmpeg: inspect_tool(&ffmpeg_names),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let download_directory = app
                .path()
                .download_dir()
                .unwrap_or_else(|_| default_download_directory());
            let state = AppState::load(app_data_dir.join("app-state.json"), download_directory)
                .map_err(std::io::Error::other)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            get_engine_status
        ])
        .run(tauri::generate_context!())
        .expect("failed to run SoundCloud Downloader");
}

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn settings_reject_invalid_values() {
        let mut settings = AppSettings::default();
        settings.max_concurrent = 0;
        assert!(settings.validate().is_err());
        settings.max_concurrent = 2;
        settings.audio_quality = "lossless".into();
        assert!(settings.validate().is_err());
        settings.audio_quality = "192".into();
        settings.audio_format = "wav".into();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn settings_accept_supported_values() {
        let settings = AppSettings::default();
        assert!(settings.validate().is_ok());
    }
}
