use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Condvar, Mutex,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};
use url::Url;

static JOB_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub download_directory: String,
    pub yt_dlp_path: String,
    pub audio_format: String,
    pub audio_quality: String,
    pub max_concurrent: usize,
    pub language: String,
    pub duplicate_handling: String,
    pub embed_artwork: bool,
    pub embed_metadata: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            download_directory: default_download_directory().to_string_lossy().into_owned(),
            yt_dlp_path: String::new(),
            audio_format: "original".into(),
            audio_quality: "192".into(),
            max_concurrent: 2,
            language: "ko".into(),
            duplicate_handling: "skip".into(),
            embed_artwork: false,
            embed_metadata: false,
        }
    }
}

impl AppSettings {
    fn validate(&self) -> Result<(), String> {
        if self.download_directory.trim().is_empty() {
            return Err("다운로드 폴더를 입력하세요.".into());
        }
        if !Path::new(&self.download_directory).is_absolute() {
            return Err("다운로드 폴더는 절대 경로여야 합니다.".into());
        }
        if !self.yt_dlp_path.trim().is_empty() && !Path::new(self.yt_dlp_path.trim()).is_absolute()
        {
            return Err("yt-dlp 실행 파일 경로는 절대 경로여야 합니다.".into());
        }
        if !matches!(self.audio_format.as_str(), "original" | "mp3") {
            return Err("지원하지 않는 오디오 형식입니다.".into());
        }
        if !matches!(self.audio_quality.as_str(), "128" | "192" | "256" | "320") {
            return Err("음질은 128, 192, 256, 320 kbps 중에서 선택하세요.".into());
        }
        if !(1..=8).contains(&self.max_concurrent) {
            return Err("동시 작업 수는 1~8개로 설정해야 합니다.".into());
        }
        if !matches!(self.language.as_str(), "ko" | "en") {
            return Err("지원하지 않는 앱 언어입니다.".into());
        }
        if !matches!(
            self.duplicate_handling.as_str(),
            "skip" | "overwrite" | "rename"
        ) {
            return Err("중복 파일 처리 방식이 올바르지 않습니다.".into());
        }
        Ok(())
    }
}

fn migrate_settings_value(value: &mut serde_json::Value) {
    if let Some(settings) = value
        .get_mut("settings")
        .and_then(serde_json::Value::as_object_mut)
    {
        if !settings.contains_key("duplicateHandling") {
            let old_overwrite = settings
                .get("overwriteFiles")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            settings.insert(
                "duplicateHandling".into(),
                serde_json::Value::String(if old_overwrite { "overwrite" } else { "skip" }.into()),
            );
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(default)]
struct Store {
    settings: AppSettings,
    jobs: Vec<DownloadJob>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Downloading,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetadata {
    pub url: String,
    pub title: String,
    pub source_id: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<u64>,
    pub selected: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistAnalysis {
    pub tracks: Vec<TrackMetadata>,
    pub playlist_name: Option<String>,
    pub total_entries: usize,
    pub skipped_entries: usize,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalysisProgress {
    id: String,
    processed: usize,
    total: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadJob {
    pub id: String,
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub playlist_name: Option<String>,
    #[serde(default = "default_duplicate_handling")]
    pub duplicate_handling: String,
    pub source_id: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<u64>,
    pub status: JobStatus,
    pub progress: f64,
    pub speed: Option<String>,
    pub eta: Option<String>,
    pub error: Option<String>,
    pub output_path: Option<String>,
    #[serde(default)]
    cancel_requested: bool,
    #[serde(default)]
    attempt: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryFile {
    path: String,
    name: String,
    size: u64,
    modified: Option<u64>,
}

fn default_duplicate_handling() -> String {
    "skip".into()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueueResult {
    jobs: Vec<DownloadJob>,
    skipped_duplicates: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsSummary {
    app_version: &'static str,
    operating_system: &'static str,
    architecture: &'static str,
    generated_at_unix: u64,
    yt_dlp_available: bool,
    yt_dlp_version: Option<String>,
    ffmpeg_available: bool,
    ffmpeg_version: Option<String>,
    queued_jobs: usize,
    downloading_jobs: usize,
    completed_jobs: usize,
    failed_jobs: usize,
    cancelled_jobs: usize,
}

struct AppState {
    store_path: PathBuf,
    diagnostics_path: PathBuf,
    store: Mutex<Store>,
    children: Mutex<HashMap<String, Child>>,
    analysis_children: Mutex<HashMap<String, Child>>,
    active_analyses: Mutex<HashSet<String>>,
    cancelled_analyses: Mutex<HashSet<String>>,
    active_workers: Mutex<HashSet<String>>,
    shutting_down: AtomicBool,
    slot_changed: Condvar,
}

impl AppState {
    fn load(store_path: PathBuf, download_directory: PathBuf) -> Result<Self, String> {
        let store = match fs::read(&store_path) {
            Ok(contents) => {
                let mut value = serde_json::from_slice::<serde_json::Value>(&contents)
                    .map_err(|error| format!("저장된 설정을 읽을 수 없습니다: {error}"))?;
                migrate_settings_value(&mut value);
                serde_json::from_value::<Store>(value)
                    .map_err(|error| format!("저장된 설정을 읽을 수 없습니다: {error}"))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut store = Store::default();
                store.settings.download_directory =
                    download_directory.to_string_lossy().into_owned();
                store
            }
            Err(error) => return Err(format!("앱 데이터 파일을 읽을 수 없습니다: {error}")),
        };
        store.settings.validate()?;
        let mut store = store;
        for job in &mut store.jobs {
            if job.status == JobStatus::Downloading {
                job.status = JobStatus::Failed;
                job.error =
                    Some("앱이 종료되어 작업이 중단되었습니다. 다시 시도할 수 있습니다.".into());
                job.cancel_requested = false;
            }
        }

        let state = Self {
            diagnostics_path: store_path.with_file_name("diagnostics.log"),
            store_path,
            store: Mutex::new(store),
            children: Mutex::new(HashMap::new()),
            analysis_children: Mutex::new(HashMap::new()),
            active_analyses: Mutex::new(HashSet::new()),
            cancelled_analyses: Mutex::new(HashSet::new()),
            active_workers: Mutex::new(HashSet::new()),
            shutting_down: AtomicBool::new(false),
            slot_changed: Condvar::new(),
        };
        state.persist()?;
        state.append_diagnostic("Application state loaded");
        Ok(state)
    }

    fn append_diagnostic(&self, message: &str) {
        const MAX_DIAGNOSTIC_BYTES: u64 = 512 * 1024;
        if self
            .diagnostics_path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > MAX_DIAGNOSTIC_BYTES)
        {
            if let Err(error) = fs::remove_file(&self.diagnostics_path) {
                eprintln!("Failed to rotate diagnostics log: {error}");
            }
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.diagnostics_path)
        {
            Ok(mut file) => {
                if let Err(error) = writeln!(file, "{timestamp} {message}") {
                    eprintln!("Failed to append diagnostics log: {error}");
                }
            }
            Err(error) => eprintln!("Failed to open diagnostics log: {error}"),
        }
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

    fn jobs(&self) -> Result<Vec<DownloadJob>, String> {
        self.store
            .lock()
            .map(|store| store.jobs.clone())
            .map_err(|_| "다운로드 기록을 읽을 수 없습니다.".into())
    }

    fn update_job(
        &self,
        id: &str,
        persist: bool,
        update: impl FnOnce(&mut DownloadJob),
    ) -> Result<DownloadJob, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "다운로드 작업 잠금에 실패했습니다.".to_string())?;
        let index = store
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| "다운로드 작업을 찾을 수 없습니다.".to_string())?;
        let previous = store.jobs[index].clone();
        update(&mut store.jobs[index]);
        let current = store.jobs[index].clone();

        if persist {
            if let Err(error) = self.persist_store(&store) {
                store.jobs[index] = previous;
                return Err(error);
            }
        }
        Ok(current)
    }

    fn claim_next_job(&self) -> Result<Option<DownloadJob>, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "다운로드 작업 잠금에 실패했습니다.".to_string())?;
        loop {
            if self.shutting_down.load(Ordering::Acquire) {
                return Ok(None);
            }

            let mut active_workers = self
                .active_workers
                .lock()
                .map_err(|_| "다운로드 실행 상태 잠금에 실패했습니다.".to_string())?;
            let next_index = store.jobs.iter().position(|job| {
                job.status == JobStatus::Queued && !active_workers.contains(&job.id)
            });
            if active_workers.len() < store.settings.max_concurrent {
                if let Some(index) = next_index {
                    let previous = store.jobs[index].clone();
                    active_workers.insert(previous.id.clone());
                    store.jobs[index].status = JobStatus::Downloading;
                    store.jobs[index].error = None;
                    store.jobs[index].progress = 0.0;
                    store.jobs[index].speed = None;
                    store.jobs[index].eta = None;
                    if let Err(error) = self.persist_store(&store) {
                        store.jobs[index] = previous;
                        active_workers.remove(&store.jobs[index].id);
                        return Err(error);
                    }
                    self.append_diagnostic(&format!(
                        "Download started: job {}",
                        store.jobs[index].id
                    ));
                    return Ok(Some(store.jobs[index].clone()));
                }
            }
            drop(active_workers);
            store = self
                .slot_changed
                .wait(store)
                .map_err(|_| "다운로드 대기열 잠금에 실패했습니다.".to_string())?;
        }
    }

    fn release_worker(&self, id: &str) {
        if let Ok(_store) = self.store.lock() {
            if let Ok(mut active_workers) = self.active_workers.lock() {
                active_workers.remove(id);
            }
        }
        self.slot_changed.notify_all();
    }

    fn terminate_running_downloads(&self) {
        self.shutting_down.store(true, Ordering::Release);
        self.slot_changed.notify_all();
        let Ok(mut children) = self.children.lock() else {
            eprintln!("Failed to lock running download processes during shutdown");
            return;
        };
        for (id, child) in children.iter_mut() {
            match terminate_child_tree(child) {
                Ok(()) => {
                    if let Err(error) = child.wait() {
                        eprintln!("Failed to wait for download process {id}: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("Failed to stop download process {id} during shutdown: {error}");
                }
            }
        }
        children.clear();
        if let Ok(mut children) = self.analysis_children.lock() {
            for (id, child) in children.iter_mut() {
                match terminate_child_tree(child) {
                    Ok(()) => {
                        if let Err(error) = child.wait() {
                            eprintln!("Failed to wait for playlist analysis {id}: {error}");
                        }
                    }
                    Err(error) => {
                        eprintln!("Failed to stop playlist analysis {id}: {error}");
                    }
                }
            }
            children.clear();
        }
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Ok(children) = self.children.get_mut() {
            for (id, child) in children {
                if let Err(error) = terminate_child_tree(child) {
                    eprintln!("Failed to stop download process {id} during shutdown: {error}");
                }
            }
            if let Ok(children) = self.analysis_children.get_mut() {
                for (id, child) in children {
                    if let Err(error) = terminate_child_tree(child) {
                        eprintln!("Failed to stop playlist analysis {id}: {error}");
                    }
                }
            }
        }
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

fn inspect_tool(names: &[&str], version_argument: &str) -> ToolStatus {
    let Some(executable) = locate_executable(names) else {
        return ToolStatus {
            available: false,
            version: None,
            error: Some("실행 파일을 PATH에서 찾지 못했습니다.".into()),
        };
    };
    inspect_executable(&executable, version_argument)
}

fn inspect_executable(executable: &Path, version_argument: &str) -> ToolStatus {
    match Command::new(executable).arg(version_argument).output() {
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

fn validate_soundcloud_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() > 2_048 {
        return Err("SoundCloud URL이 너무 깁니다.".into());
    }
    let parsed =
        Url::parse(value).map_err(|_| "올바른 SoundCloud URL을 입력하세요.".to_string())?;
    let host = parsed.host_str().unwrap_or_default();
    if parsed.scheme() != "https"
        || !(host == "soundcloud.com" || host.ends_with(".soundcloud.com"))
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.path() == "/"
    {
        return Err("HTTPS SoundCloud 트랙 또는 플레이리스트 URL만 사용할 수 있습니다.".into());
    }
    Ok(parsed.to_string())
}

fn yt_dlp_executable(configured_path: &str) -> Result<PathBuf, String> {
    let configured_path = configured_path.trim();
    if !configured_path.is_empty() {
        let configured = PathBuf::from(configured_path);
        if !configured.is_absolute() {
            return Err("yt-dlp 실행 파일 경로는 절대 경로여야 합니다.".into());
        }
        if !configured.is_file() {
            return Err("설정한 yt-dlp 실행 파일을 찾을 수 없습니다.".into());
        }
        return Ok(configured);
    }

    #[cfg(windows)]
    let names = ["yt-dlp.exe", "yt-dlp"];
    #[cfg(not(windows))]
    let names = ["yt-dlp"];

    locate_executable(&names)
        .ok_or_else(|| "yt-dlp를 찾을 수 없습니다. 설치 후 PATH에 추가하세요.".into())
}

fn ffmpeg_executable() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let names = ["ffmpeg.exe", "ffmpeg"];
    #[cfg(not(windows))]
    let names = ["ffmpeg"];

    locate_executable(&names)
        .ok_or_else(|| "FFmpeg가 필요합니다. 설치 후 PATH에 추가하세요.".into())
}

fn error_excerpt(output: &[u8]) -> String {
    let text = String::from_utf8_lossy(output);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|line| {
            let line: String = line.chars().take(400).collect();
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn metadata_arguments(url: &str) -> [&str; 5] {
    [
        "--ignore-config",
        "--skip-download",
        "--dump-json",
        "--",
        url,
    ]
}

fn download_performance_arguments(max_concurrent: usize) -> [String; 4] {
    let download_slots = max_concurrent.clamp(1, 8);
    [
        "--concurrent-fragments".into(),
        (8 / download_slots).to_string(),
        "--progress-delta".into(),
        "0.5".into(),
    ]
}

const MAX_ANALYSIS_ENTRIES: usize = 20_000;

enum AnalysisMessage {
    Stdout(String),
    Stderr(String),
    StdoutClosed,
    StderrClosed,
    ReadError(String),
}

fn number_field(value: &serde_json::Value, field: &str) -> Option<usize> {
    value.get(field).and_then(|value| {
        value
            .as_u64()
            .map(|number| number as usize)
            .or_else(|| value.as_str().and_then(|number| number.parse().ok()))
    })
}

fn playlist_progress_from_line(line: &str) -> Option<(usize, usize)> {
    let progress = line.split_once("Downloading item ")?.1;
    let (processed, total) = progress.split_once(" of ")?;
    Some((processed.trim().parse().ok()?, total.trim().parse().ok()?))
}

fn playlist_name_from_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let sets_index = segments.iter().position(|segment| *segment == "sets")?;
    let slug = segments.get(sets_index + 1)?.trim();
    if slug.is_empty() {
        return None;
    }
    Some(slug.replace(['-', '_'], " "))
}

fn playlist_name_from_entry(entry: &serde_json::Value) -> Option<&str> {
    ["playlist_title", "playlist"]
        .iter()
        .filter_map(|field| entry.get(field).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .find(|name| !name.is_empty())
}

#[tauri::command]
async fn analyze_url(id: String, url: String, app: AppHandle) -> Result<PlaylistAnalysis, String> {
    if id.trim().is_empty() || id.len() > 128 {
        return Err("분석 작업 식별자가 올바르지 않습니다.".into());
    }
    {
        let state = app.state::<AppState>();
        let mut active = state
            .active_analyses
            .lock()
            .map_err(|_| "플레이리스트 분석 잠금에 실패했습니다.".to_string())?;
        if !active.insert(id.clone()) {
            return Err("같은 플레이리스트 분석이 이미 실행 중입니다.".into());
        }
    }

    let blocking_app = app.clone();
    let analysis_id = id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = blocking_app.state::<AppState>();
        analyze_url_blocking(&blocking_app, &state, analysis_id, url)
    })
    .await
    .map_err(|error| format!("메타데이터 분석 작업을 실행하지 못했습니다: {error}"))
    .and_then(|result| result);

    let state = app.state::<AppState>();
    stop_analysis(&state, &id);
    match state.active_analyses.lock() {
        Ok(mut active) => {
            active.remove(&id);
        }
        Err(_) => eprintln!("Failed to clear active playlist analysis {id}"),
    }
    match state.cancelled_analyses.lock() {
        Ok(mut cancelled) => {
            cancelled.remove(&id);
        }
        Err(_) => eprintln!("Failed to clear cancelled playlist analysis {id}"),
    }
    result
}

#[tauri::command]
fn cancel_playlist_analysis(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let active = state
        .active_analyses
        .lock()
        .map_err(|_| "플레이리스트 분석 상태를 확인할 수 없습니다.".to_string())?
        .contains(&id);
    if !active {
        return Err("실행 중인 플레이리스트 분석을 찾을 수 없습니다.".into());
    }

    state
        .cancelled_analyses
        .lock()
        .map_err(|_| "분석 취소 상태를 저장할 수 없습니다.".to_string())?
        .insert(id.clone());
    let mut children = state
        .analysis_children
        .lock()
        .map_err(|_| "분석 프로세스 잠금에 실패했습니다.".to_string())?;
    if let Some(child) = children.get_mut(&id) {
        terminate_child_tree(child)?;
    }
    Ok(())
}

fn stop_analysis(state: &AppState, id: &str) {
    let Ok(mut children) = state.analysis_children.lock() else {
        eprintln!("Failed to lock playlist analysis process during cleanup");
        return;
    };
    if let Some(mut child) = children.remove(id) {
        if let Err(error) = terminate_child_tree(&mut child) {
            eprintln!("Failed to stop playlist analysis {id}: {error}");
        }
        if let Err(error) = child.wait() {
            eprintln!("Failed to wait for playlist analysis {id}: {error}");
        }
    }
}

fn analyze_url_blocking(
    app: &AppHandle,
    state: &AppState,
    id: String,
    url: String,
) -> Result<PlaylistAnalysis, String> {
    let url = validate_soundcloud_url(&url)?;
    let yt_dlp_path = state
        .store
        .lock()
        .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?
        .settings
        .yt_dlp_path
        .clone();
    let mut command = Command::new(yt_dlp_executable(&yt_dlp_path)?);
    command
        .args(metadata_arguments(&url))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("yt-dlp 메타데이터 분석을 시작할 수 없습니다: {error}"))?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = terminate_child_tree(&mut child);
            return Err("yt-dlp 분석 출력을 읽을 수 없습니다.".into());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = terminate_child_tree(&mut child);
            return Err("yt-dlp 분석 오류 출력을 읽을 수 없습니다.".into());
        }
    };

    match state.analysis_children.lock() {
        Ok(mut children) => {
            children.insert(id.clone(), child);
        }
        Err(_) => {
            let mut child = child;
            let _ = terminate_child_tree(&mut child);
            return Err("분석 프로세스 잠금에 실패했습니다.".into());
        }
    }

    let processing = (|| -> Result<PlaylistAnalysis, String> {
        let cancelled_before_start = state
            .cancelled_analyses
            .lock()
            .map_err(|_| "분석 취소 상태 잠금에 실패했습니다.".to_string())?
            .contains(&id);
        if cancelled_before_start {
            if let Ok(mut children) = state.analysis_children.lock() {
                if let Some(child) = children.get_mut(&id) {
                    let _ = terminate_child_tree(child);
                }
            }
        }

        let (sender, receiver) = mpsc::sync_channel(32);
        let stdout_sender = sender.clone();
        thread::Builder::new()
            .name(format!("playlist-out-{id}"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(line) => {
                            if stdout_sender.send(AnalysisMessage::Stdout(line)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = stdout_sender.send(AnalysisMessage::ReadError(format!(
                                "yt-dlp 분석 출력을 읽지 못했습니다: {error}"
                            )));
                            break;
                        }
                    }
                }
                let _ = stdout_sender.send(AnalysisMessage::StdoutClosed);
            })
            .map_err(|error| format!("플레이리스트 출력 처리를 시작하지 못했습니다: {error}"))?;
        let stderr_sender = sender.clone();
        thread::Builder::new()
            .name(format!("playlist-err-{id}"))
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    match line {
                        Ok(line) => {
                            if stderr_sender.send(AnalysisMessage::Stderr(line)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = stderr_sender.send(AnalysisMessage::ReadError(format!(
                                "yt-dlp 분석 오류 출력을 읽지 못했습니다: {error}"
                            )));
                            break;
                        }
                    }
                }
                let _ = stderr_sender.send(AnalysisMessage::StderrClosed);
            })
            .map_err(|error| format!("플레이리스트 오류 처리를 시작하지 못했습니다: {error}"))?;
        drop(sender);

        let mut tracks = Vec::new();
        let mut playlist_name = playlist_name_from_url(&url);
        let mut seen_entries = 0usize;
        let mut processed = 0usize;
        let mut reported_total = None;
        let mut stderr_lines = Vec::new();
        let mut read_error = None;
        let mut closed_streams = 0;
        while closed_streams < 2 {
            let message = receiver
                .recv()
                .map_err(|error| format!("yt-dlp 분석 결과를 받지 못했습니다: {error}"))?;
            match message {
                AnalysisMessage::Stdout(line) if !line.trim().is_empty() => {
                    let entry: serde_json::Value =
                        serde_json::from_str(&line).map_err(|error| {
                            format!("yt-dlp 트랙 메타데이터가 올바른 JSON이 아닙니다: {error}")
                        })?;
                    if let Some(name) = playlist_name_from_entry(&entry) {
                        playlist_name = Some(name.to_string());
                    }
                    seen_entries = seen_entries.saturating_add(1);
                    processed = processed.max(seen_entries);
                    reported_total = reported_total
                        .into_iter()
                        .chain(number_field(&entry, "playlist_count"))
                        .max();
                    if let Some(track) = parse_track(&entry) {
                        tracks.push(track);
                    }
                    if seen_entries > MAX_ANALYSIS_ENTRIES {
                        return Err(format!(
                            "플레이리스트가 최대 분석 항목 수({MAX_ANALYSIS_ENTRIES})를 초과했습니다."
                        ));
                    }
                    emit_analysis_progress(app, &id, processed, reported_total.or(Some(processed)));
                }
                AnalysisMessage::Stdout(_) => {}
                AnalysisMessage::Stderr(line) => {
                    if let Some((reported_processed, total)) = playlist_progress_from_line(&line) {
                        processed = processed.max(reported_processed);
                        reported_total = Some(reported_total.unwrap_or_default().max(total));
                        emit_analysis_progress(app, &id, processed, reported_total);
                    }
                    if !line.trim().is_empty() {
                        if stderr_lines.len() == 32 {
                            stderr_lines.remove(0);
                        }
                        stderr_lines.push(line);
                    }
                }
                AnalysisMessage::StdoutClosed | AnalysisMessage::StderrClosed => {
                    closed_streams += 1;
                }
                AnalysisMessage::ReadError(error) => read_error = Some(error),
            }
        }

        let mut child = state
            .analysis_children
            .lock()
            .map_err(|_| "분석 프로세스 잠금에 실패했습니다.".to_string())?
            .remove(&id)
            .ok_or_else(|| "yt-dlp 분석 프로세스를 찾을 수 없습니다.".to_string())?;
        let status = child
            .wait()
            .map_err(|error| format!("yt-dlp 분석 종료 상태를 확인하지 못했습니다: {error}"))?;
        let was_cancelled = state
            .cancelled_analyses
            .lock()
            .map_err(|_| "분석 취소 상태 잠금에 실패했습니다.".to_string())?
            .contains(&id);
        if was_cancelled {
            return Err("플레이리스트 분석을 취소했습니다.".into());
        }
        if let Some(error) = read_error {
            return Err(error);
        }
        if tracks.is_empty() {
            if !status.success() {
                let details = error_excerpt(stderr_lines.join("\n").as_bytes());
                return Err(if details.is_empty() {
                    format!("yt-dlp 분석이 종료 코드 {status}로 실패했습니다.")
                } else {
                    format!("yt-dlp 분석에 실패했습니다:\n{details}")
                });
            }
            return Err("공개 SoundCloud 트랙을 찾지 못했습니다.".into());
        }
        let total_entries = reported_total.unwrap_or(seen_entries).max(seen_entries);
        let skipped_entries = total_entries.saturating_sub(tracks.len());
        let warning = if status.success() {
            None
        } else {
            let details = error_excerpt(stderr_lines.join("\n").as_bytes());
            Some(if details.is_empty() {
                format!(
                    "yt-dlp가 종료 코드 {status}로 끝났습니다. 일부 항목이 누락되었을 수 있습니다."
                )
            } else {
                format!("일부 트랙을 분석하지 못했습니다:\n{details}")
            })
        };
        Ok(PlaylistAnalysis {
            tracks,
            playlist_name,
            total_entries,
            skipped_entries,
            warning,
        })
    })();
    if processing.is_err() {
        stop_analysis(state, &id);
    }
    processing
}

fn emit_analysis_progress(app: &AppHandle, id: &str, processed: usize, total: Option<usize>) {
    if let Err(error) = app.emit(
        "playlist-analysis-progress",
        AnalysisProgress {
            id: id.to_string(),
            processed,
            total,
        },
    ) {
        eprintln!("Failed to emit playlist-analysis-progress event: {error}");
    }
}

fn parse_track(entry: &serde_json::Value) -> Option<TrackMetadata> {
    let title = entry
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())?;
    let track_url = ["webpage_url", "original_url", "url"]
        .iter()
        .filter_map(|field| entry.get(field).and_then(serde_json::Value::as_str))
        .find_map(|candidate| validate_soundcloud_url(candidate).ok())?;

    Some(TrackMetadata {
        url: track_url,
        title: title.to_string(),
        source_id: entry.get("id").and_then(|id| {
            id.as_str()
                .map(str::to_string)
                .or_else(|| id.as_u64().map(|id| id.to_string()))
        }),
        artist: entry
            .get("artist")
            .or_else(|| entry.get("uploader"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        duration: entry.get("duration").and_then(serde_json::Value::as_u64),
        selected: false,
    })
}

#[cfg(test)]
fn parse_tracks(root: &serde_json::Value) -> Result<Vec<TrackMetadata>, String> {
    let entries: Vec<&serde_json::Value> =
        match root.get("entries").and_then(serde_json::Value::as_array) {
            Some(entries) => entries.iter().filter(|entry| !entry.is_null()).collect(),
            None => vec![&root],
        };

    let mut tracks = Vec::new();
    for entry in entries {
        if let Some(track) = parse_track(entry) {
            tracks.push(track);
        }
    }
    if tracks.is_empty() {
        return Err("공개 SoundCloud 트랙을 찾지 못했습니다.".into());
    }
    Ok(tracks)
}

fn new_job_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!(
        "{timestamp}-{}-{}",
        std::process::id(),
        JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn emit_job(app: &AppHandle, job: &DownloadJob) {
    if let Err(error) = app.emit("download-progress", job) {
        eprintln!("Failed to emit download-progress event: {error}");
    }
}

fn start_scheduler(app: AppHandle) -> Result<(), String> {
    let scheduler_app = app.clone();
    thread::Builder::new()
        .name("download-scheduler".into())
        .spawn(move || {
            let state = scheduler_app.state::<AppState>();
            loop {
                let job = match state.claim_next_job() {
                    Ok(Some(job)) => job,
                    Ok(None) => break,
                    Err(error) => {
                        eprintln!("Failed to claim the next download: {error}");
                        let queued = state.jobs().ok().and_then(|jobs| {
                            jobs.into_iter().find(|job| job.status == JobStatus::Queued)
                        });
                        if let Some(queued) = queued {
                            let message = format!("대기열 작업을 시작할 수 없습니다: {error}");
                            let update = state.update_job(&queued.id, true, |job| {
                                job.status = JobStatus::Failed;
                                job.error = Some(message.clone());
                            });
                            let job = update.unwrap_or_else(|persist_error| {
                                eprintln!("Failed to persist queued-job error: {persist_error}");
                                state
                                    .update_job(&queued.id, false, |job| {
                                        job.status = JobStatus::Failed;
                                        job.error = Some(message);
                                    })
                                    .unwrap_or(queued)
                            });
                            emit_job(&scheduler_app, &job);
                            state.slot_changed.notify_all();
                            continue;
                        }
                        break;
                    }
                };
                let worker_app = scheduler_app.clone();
                let worker_job = job.clone();
                let job_id = job.id.clone();
                let attempt = job.attempt;
                let thread_name = format!("download-{}", job.id);
                if let Err(error) = thread::Builder::new().name(thread_name).spawn(move || {
                    let state = worker_app.state::<AppState>();
                    run_job(&worker_app, &state, worker_job);
                }) {
                    let message = format!("다운로드 작업을 시작할 수 없습니다: {error}");
                    let update = state.update_job(&job_id, true, |job| {
                        if job.attempt == attempt && job.status == JobStatus::Downloading {
                            job.status = JobStatus::Failed;
                            job.error = Some(message.clone());
                        }
                    });
                    match update {
                        Ok(job) => emit_job(&scheduler_app, &job),
                        Err(error) => {
                            eprintln!("Failed to persist thread-start failure: {error}");
                            if let Ok(job) = state.update_job(&job_id, false, |job| {
                                if job.attempt == attempt && job.status == JobStatus::Downloading {
                                    job.status = JobStatus::Failed;
                                    job.error = Some(message);
                                }
                            }) {
                                emit_job(&scheduler_app, &job);
                            }
                        }
                    }
                    state.release_worker(&job_id);
                }
            }
        })
        .map(|_| ())
        .map_err(|error| format!("다운로드 스케줄러를 시작할 수 없습니다: {error}"))
}

fn run_job(app: &AppHandle, state: &AppState, job: DownloadJob) {
    let id = job.id.as_str();
    let attempt = job.attempt;
    emit_job(app, &job);
    let result = execute_download(app, state, &job);
    let was_cancelled = state.jobs().ok().is_some_and(|jobs| {
        jobs.iter()
            .any(|current| current.id == id && current.status == JobStatus::Cancelled)
    });
    state.append_diagnostic(&format!(
        "Download {}: job {id}",
        if was_cancelled {
            "cancelled"
        } else if result.is_ok() {
            "completed"
        } else {
            "failed"
        }
    ));
    let update = state.update_job(id, true, |job| {
        if job.attempt != attempt || job.status == JobStatus::Cancelled {
            return;
        }
        match &result {
            Ok(output_path) => {
                job.status = JobStatus::Completed;
                job.progress = 100.0;
                job.output_path = output_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned());
                job.error = None;
            }
            Err(error) => {
                job.status = JobStatus::Failed;
                job.error = Some(error.clone());
            }
        }
    });
    match update {
        Ok(job) => emit_job(app, &job),
        Err(error) => {
            eprintln!("Failed to persist download job {id}: {error}");
            if let Ok(job) = state.update_job(id, false, |job| {
                if job.attempt == attempt && job.status != JobStatus::Cancelled {
                    job.status = JobStatus::Failed;
                    job.error = Some(format!("작업 기록을 저장하지 못했습니다: {error}"));
                }
            }) {
                emit_job(app, &job);
            }
        }
    }
    state.release_worker(id);
}

fn execute_download(
    app: &AppHandle,
    state: &AppState,
    job: &DownloadJob,
) -> Result<Option<PathBuf>, String> {
    let url = validate_soundcloud_url(&job.url)?;
    let settings = state
        .store
        .lock()
        .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?
        .settings
        .clone();
    let download_root = PathBuf::from(&settings.download_directory);
    fs::create_dir_all(&download_root)
        .map_err(|error| format!("다운로드 폴더를 만들 수 없습니다: {error}"))?;
    let download_root = fs::canonicalize(&download_root)
        .map_err(|error| format!("다운로드 폴더 경로를 확인할 수 없습니다: {error}"))?;
    let directory = download_output_directory(&download_root, job.playlist_name.as_deref());
    fs::create_dir_all(&directory)
        .map_err(|error| format!("플레이리스트 다운로드 폴더를 만들 수 없습니다: {error}"))?;
    let directory = fs::canonicalize(directory)
        .map_err(|error| format!("플레이리스트 다운로드 폴더를 확인할 수 없습니다: {error}"))?;
    if !directory.starts_with(&download_root) {
        return Err("플레이리스트 다운로드 폴더가 설정된 다운로드 위치를 벗어났습니다.".into());
    }

    let needs_ffmpeg =
        settings.audio_format == "mp3" || settings.embed_artwork || settings.embed_metadata;
    if needs_ffmpeg {
        let _ = ffmpeg_executable()?;
    }

    let filename = format!(
        "{} - {} [{}]",
        safe_filename_component(job.artist.as_deref().unwrap_or(""), 40),
        safe_filename_component(&job.title, 100),
        safe_filename_component(job.source_id.as_deref().unwrap_or(&job.id), 40)
    );
    let filename = if job.duplicate_handling == "rename" {
        format!("{filename} ({})", safe_filename_component(&job.id, 40))
    } else {
        filename
    };
    let output_template = directory.join(format!("{filename}.%(ext)s"));
    let mut command = Command::new(yt_dlp_executable(&settings.yt_dlp_path)?);
    command
        .args(download_performance_arguments(settings.max_concurrent))
        .args([
            "--ignore-config",
            "--no-playlist",
            "--continue",
            "--newline",
            "--no-color",
            "--progress-template",
            "download:__SC_PROGRESS__%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s",
            "--format",
            "bestaudio",
        ])
        .arg("--output")
        .arg(&output_template);

    if job.duplicate_handling == "overwrite" {
        command.arg("--force-overwrites");
    } else {
        command.arg("--no-overwrites");
    }
    if settings.audio_format == "mp3" {
        command
            .args([
                "--extract-audio",
                "--audio-format",
                "mp3",
                "--audio-quality",
            ])
            .arg(format!("{}K", settings.audio_quality));
    }
    if settings.embed_artwork {
        command.arg("--embed-thumbnail");
    }
    if settings.embed_metadata {
        command.arg("--embed-metadata");
    }
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command
        .arg("--")
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("yt-dlp를 시작할 수 없습니다: {error}"))?;
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = terminate_child_tree(&mut child);
            return Err("yt-dlp 출력 스트림을 읽을 수 없습니다.".into());
        }
    };
    {
        let mut children = match state.children.lock() {
            Ok(children) => children,
            Err(_) => {
                let _ = terminate_child_tree(&mut child);
                return Err("다운로드 프로세스 잠금에 실패했습니다.".into());
            }
        };
        children.insert(job.id.clone(), child);
    }

    let cancel_requested = match state.store.lock() {
        Ok(store) => store
            .jobs
            .iter()
            .find(|current| current.id == job.id)
            .is_some_and(|current| current.attempt == job.attempt && current.cancel_requested),
        Err(_) => {
            if let Ok(mut children) = state.children.lock() {
                if let Some(mut child) = children.remove(&job.id) {
                    let _ = terminate_child_tree(&mut child);
                }
            }
            return Err("다운로드 작업 잠금에 실패했습니다.".into());
        }
    };
    let mut process_control_error = None;
    if cancel_requested {
        if let Some(child) = state
            .children
            .lock()
            .map_err(|_| "다운로드 프로세스 잠금에 실패했습니다.".to_string())?
            .get_mut(&job.id)
        {
            process_control_error = terminate_child_tree(child).err();
        }
    }

    let mut stderr_reader = BufReader::new(stderr);
    let mut line = String::new();
    let mut error_lines = Vec::new();
    let mut stream_error = None;
    loop {
        line.clear();
        let bytes = match stderr_reader.read_line(&mut line) {
            Ok(bytes) => bytes,
            Err(error) => {
                stream_error = Some(format!("yt-dlp 출력을 읽을 수 없습니다: {error}"));
                break;
            }
        };
        if bytes == 0 {
            break;
        }
        let output = line.trim();
        if let Some(progress) = output.strip_prefix("__SC_PROGRESS__") {
            update_progress(state, app, &job.id, job.attempt, progress);
        } else if !output.is_empty() {
            if error_lines.len() == 8 {
                error_lines.remove(0);
            }
            error_lines.push(output.chars().take(400).collect::<String>());
        }
    }

    let mut child = state
        .children
        .lock()
        .map_err(|_| "다운로드 프로세스 잠금에 실패했습니다.".to_string())?
        .remove(&job.id)
        .ok_or_else(|| "다운로드 프로세스를 찾을 수 없습니다.".to_string())?;
    if stream_error.is_some() {
        let _ = terminate_child_tree(&mut child);
    }
    let status = child
        .wait()
        .map_err(|error| format!("yt-dlp 종료 상태를 확인하지 못했습니다: {error}"))?;
    if let Some(error) = stream_error.or(process_control_error) {
        return Err(error);
    }
    if !status.success() {
        let details = error_lines.join("\n");
        return Err(if details.is_empty() {
            format!("yt-dlp가 종료 코드 {status}로 실패했습니다.")
        } else {
            format!("yt-dlp 다운로드에 실패했습니다:\n{details}")
        });
    }

    find_output_file(&directory, &filename)
        .map(Some)
        .ok_or_else(|| "yt-dlp는 성공했지만 오디오 파일을 찾지 못했습니다.".into())
}

fn terminate_child_tree(child: &mut Child) -> Result<(), String> {
    if child
        .try_wait()
        .map_err(|error| format!("다운로드 프로세스 상태를 확인하지 못했습니다: {error}"))?
        .is_some()
    {
        return Ok(());
    }

    #[cfg(windows)]
    {
        let process_id = child.id().to_string();
        let result = Command::new("taskkill")
            .args(["/PID", &process_id, "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if result.is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }

    #[cfg(unix)]
    {
        let process_group = -(child.id() as libc::pid_t);
        let result = unsafe { libc::kill(process_group, libc::SIGKILL) };
        if result == 0 {
            return Ok(());
        }
    }

    child
        .kill()
        .map_err(|error| format!("다운로드 프로세스 트리를 종료하지 못했습니다: {error}"))
}

fn update_progress(state: &AppState, app: &AppHandle, id: &str, attempt: u64, value: &str) {
    let fields = value.split('|').collect::<Vec<_>>();
    if fields.len() != 3 {
        return;
    }
    let percent = fields[0].trim().trim_end_matches('%').parse::<f64>();
    let Ok(percent) = percent else {
        return;
    };
    if let Ok(job) = state.update_job(id, false, |job| {
        if job.attempt == attempt && job.status == JobStatus::Downloading {
            job.progress = percent.clamp(0.0, 100.0);
            job.speed = (!fields[1].trim().is_empty()).then(|| fields[1].trim().to_string());
            job.eta = (!fields[2].trim().is_empty()).then(|| fields[2].trim().to_string());
        }
    }) {
        emit_job(app, &job);
    }
}

fn safe_filename_component(value: &str, max_chars: usize) -> String {
    let mut result = value
        .chars()
        .map(|character| {
            if character.is_control() || r#"/\:*?"<>|%"#.contains(character) {
                '_'
            } else {
                character
            }
        })
        .take(max_chars)
        .collect::<String>()
        .trim_matches([' ', '.'])
        .to_string();
    if result.is_empty() {
        result = "Unknown".into();
    }
    let device_name = result
        .split('.')
        .next()
        .unwrap_or(&result)
        .to_ascii_uppercase();
    let numbered_device = device_name
        .strip_prefix("COM")
        .or_else(|| device_name.strip_prefix("LPT"))
        .is_some_and(|number| number.len() == 1 && matches!(number.as_bytes()[0], b'1'..=b'9'));
    if matches!(device_name.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered_device {
        result.insert(0, '_');
    }
    result
}

fn download_output_directory(root: &Path, playlist_name: Option<&str>) -> PathBuf {
    playlist_name
        .filter(|name| !name.trim().is_empty())
        .map(|name| root.join(safe_filename_component(name, 100)))
        .unwrap_or_else(|| root.to_path_buf())
}

fn find_output_file(directory: &Path, filename: &str) -> Option<PathBuf> {
    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(&format!("{filename}."))
                            && is_audio_file(path)
                            && !name.ends_with(".part")
                    })
        })
}

fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "aac" | "flac" | "m4a" | "mp3" | "ogg" | "opus" | "wav" | "webm"
            )
        })
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
    if !settings.yt_dlp_path.trim().is_empty() {
        let executable = yt_dlp_executable(&settings.yt_dlp_path)?;
        let status = inspect_executable(&executable, "--version");
        if !status.available {
            return Err(status
                .error
                .unwrap_or_else(|| "설정한 yt-dlp 실행 파일을 확인할 수 없습니다.".into()));
        }
    }
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

fn write_user_selected_file(path: &str, contents: &[u8]) -> Result<(), String> {
    let destination = PathBuf::from(path);
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err("선택한 파일 경로가 올바르지 않습니다.".into());
    }
    let parent = destination
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| "선택한 파일의 폴더에 접근할 수 없습니다.".to_string())?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "선택한 파일 이름이 올바르지 않습니다.".to_string())?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    fs::write(&temporary, contents)
        .map_err(|error| format!("파일을 임시 저장할 수 없습니다: {error}"))?;
    replace_file(&temporary, &destination)
}

fn protect_app_data_targets(path: &str, state: &AppState) -> Result<(), String> {
    let destination = PathBuf::from(path);
    let name = destination
        .file_name()
        .ok_or_else(|| "선택한 파일 이름이 올바르지 않습니다.".to_string())?;
    let parent = destination
        .parent()
        .ok_or_else(|| "선택한 파일 폴더가 올바르지 않습니다.".to_string())?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("선택한 파일 폴더를 확인할 수 없습니다: {error}"))?;
    let normalized_destination =
        fs::canonicalize(&destination).unwrap_or_else(|_| canonical_parent.join(name));
    for protected in [&state.store_path, &state.diagnostics_path] {
        let normalized_protected = fs::canonicalize(protected).unwrap_or_else(|_| {
            protected
                .parent()
                .and_then(|parent| fs::canonicalize(parent).ok())
                .zip(protected.file_name())
                .map(|(parent, name)| parent.join(name))
                .unwrap_or_else(|| protected.clone())
        });
        #[cfg(windows)]
        let same_path = normalized_destination
            .to_string_lossy()
            .eq_ignore_ascii_case(&normalized_protected.to_string_lossy());
        #[cfg(not(windows))]
        let same_path = normalized_destination == normalized_protected;
        if same_path {
            return Err("앱 내부 데이터 파일을 내보내기 대상으로 선택할 수 없습니다.".into());
        }
    }
    Ok(())
}

fn validate_settings_for_import(settings: &AppSettings) -> Result<(), String> {
    settings.validate()?;
    if !settings.yt_dlp_path.trim().is_empty() {
        let executable = yt_dlp_executable(&settings.yt_dlp_path)?;
        let status = inspect_executable(&executable, "--version");
        if !status.available {
            return Err(status
                .error
                .unwrap_or_else(|| "설정 백업의 yt-dlp 경로를 확인할 수 없습니다.".into()));
        }
    }
    Ok(())
}

#[tauri::command]
fn export_settings_backup(path: String, state: State<'_, AppState>) -> Result<(), String> {
    protect_app_data_targets(&path, &state)?;
    let settings = state
        .store
        .lock()
        .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?
        .settings
        .clone();
    let contents = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("설정을 JSON으로 변환할 수 없습니다: {error}"))?;
    write_user_selected_file(&path, &contents)
}

#[tauri::command]
fn restore_settings_backup(
    path: String,
    state: State<'_, AppState>,
) -> Result<AppSettings, String> {
    let contents =
        fs::read(&path).map_err(|error| format!("설정 백업을 읽을 수 없습니다: {error}"))?;
    if contents.len() > 256 * 1024 {
        return Err("설정 백업 파일이 너무 큽니다.".into());
    }
    let mut value = serde_json::from_slice::<serde_json::Value>(&contents)
        .map_err(|error| format!("설정 백업 형식이 올바르지 않습니다: {error}"))?;
    if let Some(settings) = value.as_object_mut() {
        let old_overwrite = settings
            .get("overwriteFiles")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if !settings.contains_key("duplicateHandling") {
            settings.insert(
                "duplicateHandling".into(),
                serde_json::Value::String(if old_overwrite { "overwrite" } else { "skip" }.into()),
            );
        }
    }
    let settings = serde_json::from_value::<AppSettings>(value)
        .map_err(|error| format!("설정 백업 형식이 올바르지 않습니다: {error}"))?;
    validate_settings_for_import(&settings)?;
    let mut store = state
        .store
        .lock()
        .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?;
    let previous = store.settings.clone();
    store.settings = settings;
    if let Err(error) = state.persist_store(&store) {
        store.settings = previous;
        return Err(error);
    }
    Ok(store.settings.clone())
}

#[tauri::command]
fn export_diagnostics(path: String, state: State<'_, AppState>) -> Result<(), String> {
    protect_app_data_targets(&path, &state)?;
    let (settings, jobs) = {
        let store = state
            .store
            .lock()
            .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?;
        (store.settings.clone(), store.jobs.clone())
    };
    #[cfg(windows)]
    let (yt_dlp_names, ffmpeg_names) = (["yt-dlp.exe", "yt-dlp"], ["ffmpeg.exe", "ffmpeg"]);
    #[cfg(not(windows))]
    let (yt_dlp_names, ffmpeg_names) = (["yt-dlp", "yt-dlp"], ["ffmpeg", "ffmpeg"]);
    let yt_dlp = if settings.yt_dlp_path.trim().is_empty() {
        inspect_tool(&yt_dlp_names, "--version")
    } else {
        yt_dlp_executable(&settings.yt_dlp_path)
            .map(|path| inspect_executable(&path, "--version"))
            .unwrap_or(ToolStatus {
                available: false,
                version: None,
                error: Some("Configured executable could not be inspected.".into()),
            })
    };
    let ffmpeg = inspect_tool(&ffmpeg_names, "-version");
    let summary = DiagnosticsSummary {
        app_version: env!("CARGO_PKG_VERSION"),
        operating_system: env::consts::OS,
        architecture: env::consts::ARCH,
        generated_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default(),
        yt_dlp_available: yt_dlp.available,
        yt_dlp_version: yt_dlp.version,
        ffmpeg_available: ffmpeg.available,
        ffmpeg_version: ffmpeg.version,
        queued_jobs: jobs
            .iter()
            .filter(|job| job.status == JobStatus::Queued)
            .count(),
        downloading_jobs: jobs
            .iter()
            .filter(|job| job.status == JobStatus::Downloading)
            .count(),
        completed_jobs: jobs
            .iter()
            .filter(|job| job.status == JobStatus::Completed)
            .count(),
        failed_jobs: jobs
            .iter()
            .filter(|job| job.status == JobStatus::Failed)
            .count(),
        cancelled_jobs: jobs
            .iter()
            .filter(|job| job.status == JobStatus::Cancelled)
            .count(),
    };
    let serialized = serde_json::to_string_pretty(&summary)
        .map_err(|error| format!("진단 정보를 JSON으로 변환할 수 없습니다: {error}"))?;
    let log_contents = fs::read_to_string(&state.diagnostics_path)
        .map_err(|error| format!("진단 로그를 읽을 수 없습니다: {error}"))?;
    let recent_logs = log_contents
        .lines()
        .rev()
        .take(200)
        .collect::<VecDeque<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let report = format!(
        "SC-dlp diagnostic report\n\n{serialized}\n\nRecent application log (may contain job identifiers):\n{recent_logs}\n"
    );
    write_user_selected_file(&path, report.as_bytes())
}

#[tauri::command]
fn get_engine_status(state: State<'_, AppState>) -> Result<EngineStatus, String> {
    #[cfg(windows)]
    let (yt_dlp_names, ffmpeg_names) = (["yt-dlp.exe", "yt-dlp"], ["ffmpeg.exe", "ffmpeg"]);
    #[cfg(not(windows))]
    let (yt_dlp_names, ffmpeg_names) = (["yt-dlp", "yt-dlp"], ["ffmpeg", "ffmpeg"]);

    let configured_path = state
        .store
        .lock()
        .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?
        .settings
        .yt_dlp_path
        .clone();
    let yt_dlp = if configured_path.trim().is_empty() {
        inspect_tool(&yt_dlp_names, "--version")
    } else {
        match yt_dlp_executable(&configured_path) {
            Ok(path) => inspect_executable(&path, "--version"),
            Err(error) => ToolStatus {
                available: false,
                version: None,
                error: Some(error),
            },
        }
    };

    Ok(EngineStatus {
        yt_dlp,
        ffmpeg: inspect_tool(&ffmpeg_names, "-version"),
    })
}

#[tauri::command]
fn get_download_jobs(state: State<'_, AppState>) -> Result<Vec<DownloadJob>, String> {
    state.jobs()
}

fn reorder_queued_jobs(
    jobs: &mut [DownloadJob],
    id: &str,
    direction: &str,
) -> Result<bool, String> {
    let offset = match direction {
        "up" => -1,
        "down" => 1,
        _ => return Err("작업 이동 방향이 올바르지 않습니다.".into()),
    };
    let queued_indices = jobs
        .iter()
        .enumerate()
        .filter_map(|(index, job)| (job.status == JobStatus::Queued).then_some(index))
        .collect::<Vec<_>>();
    let queue_position = queued_indices
        .iter()
        .position(|index| jobs[*index].id == id)
        .ok_or_else(|| "대기 중인 작업만 순서를 변경할 수 있습니다.".to_string())?;
    let target_position = queue_position as isize + offset;
    if !(0..queued_indices.len() as isize).contains(&target_position) {
        return Ok(false);
    }
    jobs.swap(
        queued_indices[queue_position],
        queued_indices[target_position as usize],
    );
    Ok(true)
}

fn remove_jobs_by_id(
    jobs: &mut Vec<DownloadJob>,
    selected: &HashSet<String>,
) -> Result<(), String> {
    if selected.is_empty()
        || selected
            .iter()
            .any(|id| !jobs.iter().any(|job| &job.id == id))
    {
        return Err("선택한 작업 기록 중 찾을 수 없는 항목이 있습니다.".into());
    }
    if jobs
        .iter()
        .any(|job| selected.contains(&job.id) && job.status == JobStatus::Downloading)
    {
        return Err("다운로드 중인 작업은 기록에서 제거할 수 없습니다. 먼저 취소하세요.".into());
    }
    jobs.retain(|job| !selected.contains(&job.id));
    Ok(())
}

#[tauri::command]
fn move_queued_job(
    id: String,
    direction: String,
    state: State<'_, AppState>,
) -> Result<Vec<DownloadJob>, String> {
    let mut store = state
        .store
        .lock()
        .map_err(|_| "다운로드 대기열 잠금에 실패했습니다.".to_string())?;
    let previous_jobs = store.jobs.clone();
    if !reorder_queued_jobs(&mut store.jobs, &id, &direction)? {
        return Ok(previous_jobs);
    }
    if let Err(error) = state.persist_store(&store) {
        store.jobs = previous_jobs;
        return Err(error);
    }
    Ok(store.jobs.clone())
}

#[tauri::command]
fn remove_download_jobs(
    ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<DownloadJob>, String> {
    if ids.is_empty() {
        return Err("삭제할 작업 기록을 선택하세요.".into());
    }
    let selected = ids.into_iter().collect::<HashSet<_>>();
    let mut store = state
        .store
        .lock()
        .map_err(|_| "다운로드 대기열 잠금에 실패했습니다.".to_string())?;
    let previous_jobs = store.jobs.clone();
    remove_jobs_by_id(&mut store.jobs, &selected)?;
    if let Err(error) = state.persist_store(&store) {
        store.jobs = previous_jobs;
        return Err(error);
    }
    Ok(store.jobs.clone())
}

fn library_track_ids(directory: &Path) -> Result<HashSet<String>, String> {
    if !directory.is_dir() {
        return Ok(HashSet::new());
    }
    let files = collect_library_files(directory)?;
    Ok(files
        .into_iter()
        .filter_map(|file| {
            let filename = Path::new(&file.name).file_stem()?.to_str()?;
            let start = filename.rfind('[')? + 1;
            let end = filename[start..].find(']')? + start;
            let id = filename[start..end].trim();
            (!id.is_empty()).then(|| id.to_string())
        })
        .collect())
}

fn track_key(track: &TrackMetadata) -> String {
    track
        .source_id
        .as_deref()
        .map(|id| format!("id:{}", safe_filename_component(id, 40)))
        .unwrap_or_else(|| format!("url:{}", track.url))
}

fn same_track(track: &TrackMetadata, job: &DownloadJob) -> bool {
    match (&track.source_id, &job.source_id) {
        (Some(source_id), Some(job_source_id)) => source_id == job_source_id,
        _ => track.url == job.url,
    }
}

fn track_job_is_pending(job: &DownloadJob) -> bool {
    matches!(job.status, JobStatus::Queued | JobStatus::Downloading)
}

fn track_already_queued_or_downloaded(track: &TrackMetadata, jobs: &[DownloadJob]) -> bool {
    jobs.iter().any(|job| {
        same_track(track, job)
            && (track_job_is_pending(job)
                || (job.status == JobStatus::Completed
                    && job
                        .output_path
                        .as_deref()
                        .is_some_and(|path| Path::new(path).is_file())))
    })
}

#[tauri::command]
fn get_downloaded_track_ids(
    tracks: Vec<TrackMetadata>,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let (directory, jobs) = {
        let store = state
            .store
            .lock()
            .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?;
        (
            PathBuf::from(&store.settings.download_directory),
            store.jobs.clone(),
        )
    };
    let ids = library_track_ids(&directory)?;
    Ok(tracks
        .iter()
        .filter_map(|track| {
            let source_id = track.source_id.as_deref()?;
            (ids.contains(&safe_filename_component(source_id, 40))
                || track_already_queued_or_downloaded(track, &jobs))
            .then(|| source_id.to_string())
        })
        .collect())
}

#[tauri::command]
fn queue_downloads(
    tracks: Vec<TrackMetadata>,
    playlist_name: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<QueueResult, String> {
    let playlist_name = match playlist_name {
        Some(name) => {
            let name = name.trim();
            if name.is_empty() {
                return Err("플레이리스트 이름이 비어 있습니다.".into());
            }
            if name.chars().count() > 500 {
                return Err("플레이리스트 이름이 너무 깁니다.".into());
            }
            Some(name.to_string())
        }
        None => None,
    };
    let selected = tracks
        .into_iter()
        .filter(|track| track.selected)
        .map(|mut track| {
            track.url = validate_soundcloud_url(&track.url)?;
            if track.title.trim().is_empty() {
                return Err("제목이 없는 트랙은 대기열에 추가할 수 없습니다.".into());
            }
            Ok(track)
        })
        .collect::<Result<Vec<_>, String>>()?;
    if selected.is_empty() {
        return Err("대기열에 추가할 트랙을 선택하세요.".into());
    }

    let (settings, existing_jobs) = {
        let store = state
            .store
            .lock()
            .map_err(|_| "다운로드 대기열 잠금에 실패했습니다.".to_string())?;
        (store.settings.clone(), store.jobs.clone())
    };
    let existing_ids = library_track_ids(Path::new(&settings.download_directory))?;
    let mut encountered = HashSet::new();
    let mut skipped_duplicates = 0;
    let selected = selected
        .into_iter()
        .filter(|track| {
            let duplicate = match settings.duplicate_handling.as_str() {
                "skip" => {
                    track_already_queued_or_downloaded(track, &existing_jobs)
                        || track.source_id.as_deref().is_some_and(|id| {
                            existing_ids.contains(&safe_filename_component(id, 40))
                        })
                        || !encountered.insert(track_key(track))
                }
                "overwrite" => {
                    existing_jobs
                        .iter()
                        .any(|job| track_job_is_pending(job) && same_track(track, job))
                        || !encountered.insert(track_key(track))
                }
                "rename" => false,
                _ => true,
            };
            if duplicate {
                skipped_duplicates += 1;
            }
            !duplicate
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(QueueResult {
            jobs: Vec::new(),
            skipped_duplicates,
        });
    }

    let jobs = selected
        .into_iter()
        .map(|track| DownloadJob {
            id: new_job_id(),
            url: track.url,
            title: track.title,
            playlist_name: playlist_name.clone(),
            duplicate_handling: settings.duplicate_handling.clone(),
            source_id: track.source_id,
            artist: track.artist,
            duration: track.duration,
            status: JobStatus::Queued,
            progress: 0.0,
            speed: None,
            eta: None,
            error: None,
            output_path: None,
            cancel_requested: false,
            attempt: 0,
        })
        .collect::<Vec<_>>();
    {
        let mut store = state
            .store
            .lock()
            .map_err(|_| "다운로드 대기열 잠금에 실패했습니다.".to_string())?;
        let original_len = store.jobs.len();
        store.jobs.extend(jobs.iter().cloned());
        if let Err(error) = state.persist_store(&store) {
            store.jobs.truncate(original_len);
            return Err(error);
        }
    }
    state.append_diagnostic(&format!(
        "Queued {} download(s); skipped {skipped_duplicates} duplicate(s)",
        jobs.len()
    ));
    for job in &jobs {
        emit_job(&app, job);
    }
    state.slot_changed.notify_all();
    Ok(QueueResult {
        jobs,
        skipped_duplicates,
    })
}

fn cancel_job_inner(app: &AppHandle, state: &AppState, id: &str) -> Result<DownloadJob, String> {
    let current = state
        .jobs()?
        .into_iter()
        .find(|job| job.id == id)
        .ok_or_else(|| "다운로드 작업을 찾을 수 없습니다.".to_string())?;
    let job = match current.status {
        JobStatus::Queued => state.update_job(id, true, |job| {
            job.status = JobStatus::Cancelled;
            job.error = None;
        })?,
        JobStatus::Downloading => {
            let job = state.update_job(id, true, |job| {
                job.status = JobStatus::Cancelled;
                job.cancel_requested = true;
                job.error = None;
            })?;
            let mut children = state
                .children
                .lock()
                .map_err(|_| "다운로드 프로세스 잠금에 실패했습니다.".to_string())?;
            if let Some(child) = children.get_mut(id) {
                terminate_child_tree(child)?;
            }
            job
        }
        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
            return Err("대기 중이거나 진행 중인 작업만 취소할 수 있습니다.".into());
        }
    };
    state.append_diagnostic(&format!("Download cancelled: job {id}"));
    emit_job(app, &job);
    state.slot_changed.notify_all();
    Ok(job)
}

#[tauri::command]
fn cancel_download(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DownloadJob, String> {
    cancel_job_inner(&app, &state, &id)
}

#[tauri::command]
fn cancel_all_downloads(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let ids = state
        .jobs()?
        .into_iter()
        .filter(|job| matches!(job.status, JobStatus::Queued | JobStatus::Downloading))
        .map(|job| job.id)
        .collect::<Vec<_>>();
    let mut errors = Vec::new();
    for id in ids {
        if let Err(error) = cancel_job_inner(&app, &state, &id) {
            errors.push(format!("{id}: {error}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "일부 다운로드를 취소하지 못했습니다:\n{}",
            errors.join("\n")
        ))
    }
}

#[tauri::command]
fn retry_download(
    id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DownloadJob, String> {
    let job = state.update_job(&id, true, |job| {
        if matches!(job.status, JobStatus::Failed | JobStatus::Cancelled) {
            job.status = JobStatus::Queued;
            job.progress = 0.0;
            job.speed = None;
            job.eta = None;
            job.error = None;
            job.output_path = None;
            job.cancel_requested = false;
            job.attempt = job.attempt.saturating_add(1);
        }
    })?;
    if job.status != JobStatus::Queued {
        return Err("실패하거나 취소된 작업만 다시 시도할 수 있습니다.".into());
    }
    state.slot_changed.notify_all();
    emit_job(&app, &job);
    Ok(job)
}

fn library_directory(state: &AppState) -> Result<PathBuf, String> {
    let directory = state
        .store
        .lock()
        .map_err(|_| "앱 설정 잠금에 실패했습니다.".to_string())?
        .settings
        .download_directory
        .clone();
    let directory = fs::canonicalize(directory)
        .map_err(|error| format!("다운로드 폴더에 접근할 수 없습니다: {error}"))?;
    if !directory.is_dir() {
        return Err("다운로드 위치가 폴더가 아닙니다.".into());
    }
    Ok(directory)
}

fn resolve_library_file(state: &AppState, path: &str) -> Result<PathBuf, String> {
    let root = library_directory(state)?;
    let file = fs::canonicalize(path)
        .map_err(|error| format!("라이브러리 파일에 접근할 수 없습니다: {error}"))?;
    if !file.starts_with(&root) || file == root || !file.is_file() || !is_audio_file(&file) {
        return Err("설정된 다운로드 폴더의 오디오 파일만 선택할 수 있습니다.".into());
    }
    Ok(file)
}

#[tauri::command]
fn get_library_files(state: State<'_, AppState>) -> Result<Vec<LibraryFile>, String> {
    let directory = library_directory(&state)?;
    collect_library_files(&directory)
}

fn collect_library_files(directory: &Path) -> Result<Vec<LibraryFile>, String> {
    let mut files = Vec::new();
    let mut pending_directories = vec![directory.to_path_buf()];
    while let Some(current_directory) = pending_directories.pop() {
        let entries = fs::read_dir(&current_directory)
            .map_err(|error| format!("라이브러리 폴더를 읽을 수 없습니다: {error}"))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("라이브러리 항목을 읽을 수 없습니다: {error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("파일 종류를 확인할 수 없습니다: {error}"))?;
            if file_type.is_dir() {
                pending_directories.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if !is_audio_file(&path) {
                continue;
            }
            let metadata = entry
                .metadata()
                .map_err(|error| format!("파일 정보를 읽을 수 없습니다: {error}"))?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());
            files.push(LibraryFile {
                path: path.to_string_lossy().into_owned(),
                name: entry.file_name().to_string_lossy().into_owned(),
                size: metadata.len(),
                modified,
            });
        }
    }
    files.sort_by_key(|file| (file.name.to_lowercase(), file.path.to_lowercase()));
    Ok(files)
}

#[tauri::command]
fn delete_library_file(path: String, state: State<'_, AppState>) -> Result<(), String> {
    let file = resolve_library_file(&state, &path)?;
    fs::remove_file(file).map_err(|error| format!("파일을 삭제할 수 없습니다: {error}"))
}

fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");

    command
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("파일 또는 폴더를 열 수 없습니다: {error}"))
}

#[tauri::command]
fn open_library_file(path: String, state: State<'_, AppState>) -> Result<(), String> {
    open_path(&resolve_library_file(&state, &path)?)
}

#[tauri::command]
fn open_library_folder(state: State<'_, AppState>) -> Result<(), String> {
    open_path(&library_directory(&state)?)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let download_directory = app
                .path()
                .download_dir()
                .unwrap_or_else(|_| default_download_directory());
            let state = AppState::load(app_data_dir.join("app-state.json"), download_directory)
                .map_err(std::io::Error::other)?;
            app.manage(state);
            start_scheduler(app.handle().clone()).map_err(std::io::Error::other)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            export_settings_backup,
            restore_settings_backup,
            export_diagnostics,
            get_engine_status,
            analyze_url,
            cancel_playlist_analysis,
            get_download_jobs,
            get_downloaded_track_ids,
            queue_downloads,
            move_queued_job,
            remove_download_jobs,
            cancel_download,
            cancel_all_downloads,
            retry_download,
            get_library_files,
            delete_library_file,
            open_library_file,
            open_library_folder
        ])
        .build(tauri::generate_context!())
        .expect("failed to build SoundCloud Downloader");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            if let Some(state) = handle.try_state::<AppState>() {
                state.terminate_running_downloads();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        collect_library_files, download_output_directory, download_performance_arguments,
        inspect_executable, library_track_ids, metadata_arguments, migrate_settings_value,
        parse_tracks, playlist_name_from_entry, playlist_name_from_url,
        playlist_progress_from_line, protect_app_data_targets, remove_jobs_by_id,
        reorder_queued_jobs, resolve_library_file, safe_filename_component,
        track_already_queued_or_downloaded, validate_soundcloud_url, yt_dlp_executable,
        AppSettings, AppState, DownloadJob, JobStatus, Store, TrackMetadata,
    };
    use std::{collections::HashSet, fs, path::PathBuf};

    fn test_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "soundcloud-downloader-test-{}-{}",
            std::process::id(),
            super::JOB_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn test_job(id: &str, status: JobStatus) -> DownloadJob {
        DownloadJob {
            id: id.into(),
            url: format!("https://soundcloud.com/artist/{id}"),
            title: id.into(),
            playlist_name: None,
            duplicate_handling: "skip".into(),
            source_id: None,
            artist: None,
            duration: None,
            status,
            progress: 0.0,
            speed: None,
            eta: None,
            error: None,
            output_path: None,
            cancel_requested: false,
            attempt: 0,
        }
    }

    #[test]
    fn settings_reject_invalid_values() {
        assert!(AppSettings {
            max_concurrent: 0,
            ..AppSettings::default()
        }
        .validate()
        .is_err());
        assert!(AppSettings {
            audio_quality: "lossless".into(),
            ..AppSettings::default()
        }
        .validate()
        .is_err());
        assert!(AppSettings {
            audio_format: "wav".into(),
            ..AppSettings::default()
        }
        .validate()
        .is_err());
        assert!(AppSettings {
            yt_dlp_path: "yt-dlp.exe".into(),
            ..AppSettings::default()
        }
        .validate()
        .is_err());
        assert!(AppSettings {
            language: "fr".into(),
            ..AppSettings::default()
        }
        .validate()
        .is_err());
        assert!(AppSettings {
            max_concurrent: 9,
            ..AppSettings::default()
        }
        .validate()
        .is_err());
        assert!(AppSettings {
            duplicate_handling: "merge".into(),
            ..AppSettings::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn settings_accept_supported_values() {
        let mut settings = AppSettings::default();
        assert!(settings.validate().is_ok());
        settings.max_concurrent = 8;
        settings.language = "en".into();
        settings.duplicate_handling = "rename".into();
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn legacy_overwrite_setting_migrates_to_duplicate_handling() {
        let mut value = serde_json::json!({ "settings": { "overwriteFiles": true } });
        migrate_settings_value(&mut value);
        let store = serde_json::from_value::<Store>(value).unwrap();
        assert_eq!(store.settings.duplicate_handling, "overwrite");
    }

    #[test]
    fn download_pipeline_uses_parallel_fragments_and_throttled_progress() {
        assert_eq!(
            download_performance_arguments(1),
            [
                "--concurrent-fragments".to_string(),
                "8".to_string(),
                "--progress-delta".to_string(),
                "0.5".to_string()
            ]
        );
        assert_eq!(download_performance_arguments(4)[1], "2");
        assert_eq!(download_performance_arguments(8)[1], "1");
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_version_check_uses_single_dash_argument() {
        use std::os::unix::fs::PermissionsExt;

        let directory = test_directory();
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("ffmpeg");
        fs::write(
            &executable,
            "#!/bin/sh\n[ \"$1\" = \"-version\" ] || { echo \"Unrecognized option '$1'\" >&2; exit 1; }\necho 'ffmpeg version test'\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

        let status = inspect_executable(&executable, "-version");
        assert!(status.available);
        assert_eq!(status.version.as_deref(), Some("ffmpeg version test"));
        assert!(status.error.is_none());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn soundcloud_urls_require_https_and_an_exact_soundcloud_domain() {
        assert!(validate_soundcloud_url("https://soundcloud.com/artist/track").is_ok());
        assert!(validate_soundcloud_url("https://on.soundcloud.com/short-link").is_ok());
        assert!(validate_soundcloud_url("http://soundcloud.com/artist/track").is_err());
        assert!(validate_soundcloud_url("https://soundcloud.com.example.org/track").is_err());
        assert!(validate_soundcloud_url("https://user@soundcloud.com/track").is_err());
    }

    #[test]
    fn metadata_parser_returns_only_public_soundcloud_tracks_unselected() {
        let response = serde_json::json!({
            "entries": [
                {
                    "id": 123,
                    "title": "Track",
                    "uploader": "Artist",
                    "duration": 83,
                    "webpage_url": "https://soundcloud.com/artist/track"
                },
                {
                    "id": "456",
                    "title": "Second Track",
                    "uploader": "Artist",
                    "webpage_url": "https://soundcloud.com/artist/second-track",
                    "url": "https://playback.media-streaming.soundcloud.cloud/track"
                },
                {
                    "id": "bad",
                    "title": "Not SoundCloud",
                    "webpage_url": "https://example.org/track"
                }
            ]
        });

        let tracks = parse_tracks(&response).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].source_id.as_deref(), Some("123"));
        assert_eq!(tracks[0].artist.as_deref(), Some("Artist"));
        assert_eq!(tracks[0].duration, Some(83));
        assert!(!tracks[0].selected);
        assert_eq!(tracks[1].source_id.as_deref(), Some("456"));
        assert_eq!(tracks[1].url, "https://soundcloud.com/artist/second-track");
        assert!(!tracks[1].selected);
    }

    #[test]
    fn playlist_analysis_arguments_resolve_track_metadata_without_downloading_audio() {
        let arguments = metadata_arguments("https://soundcloud.com/artist/sets/playlist");

        assert!(arguments.contains(&"--skip-download"));
        assert!(arguments.contains(&"--dump-json"));
        assert!(!arguments.contains(&"--flat-playlist"));
        assert_eq!(arguments[4], "https://soundcloud.com/artist/sets/playlist");
    }

    #[test]
    fn playlist_names_use_metadata_or_the_soundcloud_set_slug() {
        let entry = serde_json::json!({ "playlist_title": "My Playlist" });
        assert_eq!(playlist_name_from_entry(&entry), Some("My Playlist"));
        assert_eq!(
            playlist_name_from_url("https://soundcloud.com/artist/sets/my-playlist"),
            Some("my playlist".into())
        );
        assert_eq!(
            playlist_name_from_url("https://soundcloud.com/artist/track"),
            None
        );
    }

    #[test]
    fn playlist_analysis_progress_parses_yt_dlp_item_status() {
        assert_eq!(
            playlist_progress_from_line("[download] Downloading item 12 of 340"),
            Some((12, 340))
        );
        assert_eq!(
            playlist_progress_from_line("[info] resolving playlist"),
            None
        );
    }

    #[test]
    fn configured_yt_dlp_path_is_used_and_must_exist() {
        let directory = test_directory();
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("custom-yt-dlp");
        fs::write(&executable, "tool").unwrap();

        assert_eq!(
            yt_dlp_executable(executable.to_str().unwrap()).unwrap(),
            executable
        );
        assert!(yt_dlp_executable("relative/yt-dlp").is_err());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn filename_components_cannot_create_paths_or_windows_devices() {
        assert_eq!(safe_filename_component("../track:one", 100), "_track_one");
        assert_eq!(safe_filename_component("CON", 100), "_CON");
        assert_eq!(safe_filename_component("COM2.txt", 100), "_COM2.txt");
        assert_eq!(safe_filename_component("...", 100), "Unknown");
    }

    #[test]
    fn playlist_output_directories_stay_under_the_download_root() {
        let root = PathBuf::from("music-downloads");
        assert_eq!(
            download_output_directory(&root, Some("../outside")),
            root.join("_outside")
        );
        assert_eq!(download_output_directory(&root, None), root);
    }

    #[test]
    fn settings_survive_app_restart() {
        let directory = test_directory();
        let store_path = directory.join("app-state.json");
        let state = AppState::load(store_path.clone(), directory.join("Downloads")).unwrap();
        {
            let mut store = state.store.lock().unwrap();
            store.settings.audio_format = "mp3".into();
            store.settings.max_concurrent = 4;
            store.settings.language = "en".into();
            state.persist_store(&store).unwrap();
        }
        drop(state);

        let restored = AppState::load(store_path, directory.join("Fallback")).unwrap();
        let store = restored.store.lock().unwrap();
        assert_eq!(store.settings.audio_format, "mp3");
        assert_eq!(store.settings.max_concurrent, 4);
        assert_eq!(store.settings.language, "en");
        assert_eq!(
            store.settings.download_directory,
            directory.join("Downloads").to_string_lossy()
        );
        drop(store);
        drop(restored);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn library_paths_are_confined_to_the_configured_download_directory() {
        let directory = test_directory();
        let downloads = directory.join("Downloads");
        let outside = directory.join("outside.mp3");
        let playlist_directory = downloads.join("Playlist");
        fs::create_dir_all(&playlist_directory).unwrap();
        let inside = downloads.join("track.mp3");
        let nested = playlist_directory.join("nested.mp3");
        fs::write(&inside, b"audio").unwrap();
        fs::write(&nested, b"audio").unwrap();
        fs::write(&outside, b"audio").unwrap();
        let state = AppState::load(directory.join("app-state.json"), downloads).unwrap();

        assert!(resolve_library_file(&state, inside.to_str().unwrap()).is_ok());
        assert!(resolve_library_file(&state, nested.to_str().unwrap()).is_ok());
        assert!(resolve_library_file(&state, outside.to_str().unwrap()).is_err());
        let files = collect_library_files(&directory.join("Downloads")).unwrap();
        assert_eq!(files.len(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn library_track_ids_are_detected_for_incremental_playlist_selection() {
        let directory = test_directory();
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("Artist - Song [12345].mp3"), b"audio").unwrap();

        let ids = library_track_ids(&directory).unwrap();
        assert!(ids.contains("12345"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn existing_tracks_are_detected_only_when_the_completed_file_exists() {
        let directory = test_directory();
        fs::create_dir_all(&directory).unwrap();
        let output = directory.join("Artist - Track [123].mp3");
        fs::write(&output, b"audio").unwrap();
        let track = TrackMetadata {
            url: "https://soundcloud.com/artist/track".into(),
            title: "Track".into(),
            source_id: Some("123".into()),
            artist: Some("Artist".into()),
            duration: None,
            selected: true,
        };
        let completed_job = DownloadJob {
            id: "job-1".into(),
            url: track.url.clone(),
            title: track.title.clone(),
            playlist_name: None,
            duplicate_handling: "skip".into(),
            source_id: track.source_id.clone(),
            artist: track.artist.clone(),
            duration: None,
            status: JobStatus::Completed,
            progress: 100.0,
            speed: None,
            eta: None,
            error: None,
            output_path: Some(output.to_string_lossy().into_owned()),
            cancel_requested: false,
            attempt: 0,
        };

        assert!(track_already_queued_or_downloaded(
            &track,
            std::slice::from_ref(&completed_job)
        ));
        fs::remove_file(output).unwrap();
        assert!(!track_already_queued_or_downloaded(
            &track,
            std::slice::from_ref(&completed_job)
        ));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn queued_job_order_can_change_without_moving_active_jobs() {
        let mut jobs = vec![
            test_job("active", JobStatus::Downloading),
            test_job("first", JobStatus::Queued),
            test_job("second", JobStatus::Queued),
        ];

        assert!(reorder_queued_jobs(&mut jobs, "second", "up").unwrap());
        assert_eq!(jobs[0].id, "active");
        assert_eq!(jobs[1].id, "second");
        assert_eq!(jobs[2].id, "first");
        assert!(!reorder_queued_jobs(&mut jobs, "second", "up").unwrap());
    }

    #[test]
    fn bulk_removal_rejects_active_jobs_and_preserves_files() {
        let mut jobs = vec![
            test_job("queued", JobStatus::Queued),
            test_job("active", JobStatus::Downloading),
            test_job("done", JobStatus::Completed),
        ];
        let active_ids = HashSet::from(["active".to_string()]);
        assert!(remove_jobs_by_id(&mut jobs, &active_ids).is_err());
        assert_eq!(jobs.len(), 3);

        let removable_ids = HashSet::from(["queued".to_string(), "done".to_string()]);
        remove_jobs_by_id(&mut jobs, &removable_ids).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "active");
    }

    #[test]
    fn exports_cannot_overwrite_app_data_or_diagnostics() {
        let directory = test_directory();
        let state = AppState::load(
            directory.join("app-state.json"),
            directory.join("Downloads"),
        )
        .unwrap();

        assert!(protect_app_data_targets(state.store_path.to_str().unwrap(), &state).is_err());
        assert!(
            protect_app_data_targets(state.diagnostics_path.to_str().unwrap(), &state).is_err()
        );
        assert!(
            protect_app_data_targets(directory.join("backup.json").to_str().unwrap(), &state)
                .is_ok()
        );

        drop(state);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn scheduler_dispatches_queued_jobs_in_fifo_order_with_configured_limit() {
        let directory = test_directory();
        fs::create_dir_all(&directory).unwrap();
        let state = AppState::load(
            directory.join("app-state.json"),
            directory.join("Downloads"),
        )
        .unwrap();
        {
            let mut store = state.store.lock().unwrap();
            store.settings.max_concurrent = 8;
            store.jobs = (0..9)
                .map(|index| test_job(&format!("job-{index}"), JobStatus::Queued))
                .collect();
            state.persist_store(&store).unwrap();
        }

        let workers = (0..8)
            .map(|_| state.claim_next_job().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(state.active_workers.lock().unwrap().len(), 8);
        assert_eq!(state.jobs().unwrap()[8].status, JobStatus::Queued);

        state.release_worker(&workers[0].id);
        let next = state.claim_next_job().unwrap().unwrap();
        assert_eq!(next.id, "job-8");
        for job in workers.iter().skip(1) {
            state.release_worker(&job.id);
        }
        state.release_worker(&next.id);

        drop(state);
        fs::remove_dir_all(directory).unwrap();
    }
}
