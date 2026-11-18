use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Condvar, Mutex,
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
pub struct DownloadJob {
    pub id: String,
    pub url: String,
    pub title: String,
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

struct AppState {
    store_path: PathBuf,
    store: Mutex<Store>,
    children: Mutex<HashMap<String, Child>>,
    active_workers: Mutex<HashSet<String>>,
    shutting_down: AtomicBool,
    slot_changed: Condvar,
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
            store_path,
            store: Mutex::new(store),
            children: Mutex::new(HashMap::new()),
            active_workers: Mutex::new(HashSet::new()),
            shutting_down: AtomicBool::new(false),
            slot_changed: Condvar::new(),
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

fn yt_dlp_executable() -> Result<PathBuf, String> {
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

#[tauri::command]
async fn analyze_url(url: String) -> Result<Vec<TrackMetadata>, String> {
    tauri::async_runtime::spawn_blocking(move || analyze_url_blocking(url))
        .await
        .map_err(|error| format!("메타데이터 분석 작업을 실행하지 못했습니다: {error}"))?
}

fn analyze_url_blocking(url: String) -> Result<Vec<TrackMetadata>, String> {
    let url = validate_soundcloud_url(&url)?;
    let output = Command::new(yt_dlp_executable()?)
        .args([
            "--ignore-config",
            "--no-warnings",
            "--flat-playlist",
            "--skip-download",
            "--dump-single-json",
            "--",
            &url,
        ])
        .output()
        .map_err(|error| format!("yt-dlp 메타데이터 분석을 시작할 수 없습니다: {error}"))?;

    if !output.status.success() {
        let details = error_excerpt(&output.stderr);
        return Err(if details.is_empty() {
            format!("yt-dlp 분석이 종료 코드 {}로 실패했습니다.", output.status)
        } else {
            format!("yt-dlp 분석에 실패했습니다:\n{details}")
        });
    }

    let root: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("yt-dlp가 올바른 JSON을 반환하지 않았습니다: {error}"))?;
    parse_tracks(&root)
}

fn parse_tracks(root: &serde_json::Value) -> Result<Vec<TrackMetadata>, String> {
    let entries: Vec<&serde_json::Value> =
        match root.get("entries").and_then(serde_json::Value::as_array) {
            Some(entries) => entries.iter().filter(|entry| !entry.is_null()).collect(),
            None => vec![&root],
        };

    let mut tracks = Vec::new();
    for entry in entries {
        let title = entry
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty());
        let Some(title) = title else {
            continue;
        };
        let track_url = ["webpage_url", "original_url", "url"]
            .iter()
            .filter_map(|field| entry.get(field).and_then(serde_json::Value::as_str))
            .find_map(|candidate| validate_soundcloud_url(candidate).ok());
        let Some(track_url) = track_url else {
            continue;
        };

        tracks.push(TrackMetadata {
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
        });
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
    let directory = PathBuf::from(&settings.download_directory);
    fs::create_dir_all(&directory)
        .map_err(|error| format!("다운로드 폴더를 만들 수 없습니다: {error}"))?;

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
    let output_template = directory.join(format!("{filename}.%(ext)s"));
    let mut command = Command::new(yt_dlp_executable()?);
    command
        .args([
            "--ignore-config",
            "--no-playlist",
            "--newline",
            "--no-color",
            "--progress-template",
            "download:__SC_PROGRESS__%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s",
            "--format",
            "bestaudio",
        ])
        .arg("--output")
        .arg(&output_template);

    if settings.overwrite_files {
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
    let cancelled = state
        .jobs()?
        .into_iter()
        .find(|current| current.id == job.id)
        .is_some_and(|current| {
            current.attempt == job.attempt && current.status == JobStatus::Cancelled
        });
    if cancelled {
        remove_partial_files(&directory, &filename);
    }
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

fn remove_partial_files(directory: &Path, filename: &str) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_partial = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(&format!("{filename}.")) && name.ends_with(".part")
            });
        if is_partial {
            if let Err(error) = fs::remove_file(path) {
                eprintln!("Failed to remove a cancelled partial download: {error}");
            }
        }
    }
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
    if matches!(
        result.to_ascii_uppercase().as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "LPT1"
    ) {
        result.insert(0, '_');
    }
    result
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

#[tauri::command]
fn get_download_jobs(state: State<'_, AppState>) -> Result<Vec<DownloadJob>, String> {
    state.jobs()
}

#[tauri::command]
fn queue_downloads(
    tracks: Vec<TrackMetadata>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<DownloadJob>, String> {
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

    let jobs = selected
        .into_iter()
        .map(|track| DownloadJob {
            id: new_job_id(),
            url: track.url,
            title: track.title,
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
    for job in &jobs {
        emit_job(&app, job);
    }
    state.slot_changed.notify_all();
    Ok(jobs)
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
    let entries = fs::read_dir(&directory)
        .map_err(|error| format!("라이브러리 폴더를 읽을 수 없습니다: {error}"))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("라이브러리 항목을 읽을 수 없습니다: {error}"))?;
        if !entry
            .file_type()
            .map_err(|error| format!("파일 종류를 확인할 수 없습니다: {error}"))?
            .is_file()
        {
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
    files.sort_by_key(|file| file.name.to_lowercase());
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
            get_engine_status,
            analyze_url,
            get_download_jobs,
            queue_downloads,
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
        parse_tracks, resolve_library_file, safe_filename_component, validate_soundcloud_url,
        AppSettings, AppState, DownloadJob, JobStatus,
    };
    use std::{fs, path::PathBuf};

    fn test_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "soundcloud-downloader-test-{}-{}",
            std::process::id(),
            super::JOB_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

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
                    "id": "bad",
                    "title": "Not SoundCloud",
                    "webpage_url": "https://example.org/track"
                }
            ]
        });

        let tracks = parse_tracks(&response).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].source_id.as_deref(), Some("123"));
        assert_eq!(tracks[0].artist.as_deref(), Some("Artist"));
        assert_eq!(tracks[0].duration, Some(83));
        assert!(!tracks[0].selected);
    }

    #[test]
    fn filename_components_cannot_create_paths_or_windows_devices() {
        assert_eq!(safe_filename_component("../track:one", 100), "_track_one");
        assert_eq!(safe_filename_component("CON", 100), "_CON");
        assert_eq!(safe_filename_component("...", 100), "Unknown");
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
            state.persist_store(&store).unwrap();
        }
        drop(state);

        let restored = AppState::load(store_path, directory.join("Fallback")).unwrap();
        let store = restored.store.lock().unwrap();
        assert_eq!(store.settings.audio_format, "mp3");
        assert_eq!(store.settings.max_concurrent, 4);
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
        fs::create_dir_all(&downloads).unwrap();
        let inside = downloads.join("track.mp3");
        fs::write(&inside, b"audio").unwrap();
        fs::write(&outside, b"audio").unwrap();
        let state = AppState::load(directory.join("app-state.json"), downloads).unwrap();

        assert!(resolve_library_file(&state, inside.to_str().unwrap()).is_ok());
        assert!(resolve_library_file(&state, outside.to_str().unwrap()).is_err());
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
            store.settings.max_concurrent = 2;
            store.jobs = (0..3)
                .map(|index| DownloadJob {
                    id: format!("job-{index}"),
                    url: format!("https://soundcloud.com/artist/track-{index}"),
                    title: format!("Track {index}"),
                    source_id: None,
                    artist: None,
                    duration: None,
                    status: JobStatus::Queued,
                    progress: 0.0,
                    speed: None,
                    eta: None,
                    error: None,
                    output_path: None,
                    cancel_requested: false,
                    attempt: 0,
                })
                .collect();
            state.persist_store(&store).unwrap();
        }

        let first = state.claim_next_job().unwrap().unwrap();
        let second = state.claim_next_job().unwrap().unwrap();
        assert_eq!([first.id.as_str(), second.id.as_str()], ["job-0", "job-1"]);
        assert_eq!(state.active_workers.lock().unwrap().len(), 2);
        assert_eq!(state.jobs().unwrap()[2].status, JobStatus::Queued);

        state.release_worker(&first.id);
        let third = state.claim_next_job().unwrap().unwrap();
        assert_eq!(third.id, "job-2");
        state.release_worker(&second.id);
        state.release_worker(&third.id);

        drop(state);
        fs::remove_dir_all(directory).unwrap();
    }
}
