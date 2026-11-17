use crate::downloader::{find_ytdlp, validate_soundcloud_url};
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
const EVENT_NAME: &str = "download-progress";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadJob {
    pub id: String,
    pub url: String,
    pub title: String,
    pub status: DownloadStatus,
    pub progress: f32,
    pub speed: Option<String>,
    pub eta: Option<String>,
    pub output_path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DownloadEvent {
    job: DownloadJob,
}

#[derive(Default)]
pub struct DownloadManager {
    jobs: Arc<Mutex<Vec<DownloadJob>>>,
    processes: Arc<Mutex<HashMap<String, Arc<Mutex<Child>>>>>,
}

impl DownloadManager {
    fn update(&self, app: &AppHandle, id: &str, update: impl FnOnce(&mut DownloadJob)) {
        let snapshot = {
            let Ok(mut jobs) = self.jobs.lock() else { return };
            let Some(job) = jobs.iter_mut().find(|job| job.id == id) else { return };
            update(job);
            job.clone()
        };
        let _ = app.emit(EVENT_NAME, DownloadEvent { job: snapshot });
    }

    fn remove_process(&self, id: &str) {
        if let Ok(mut processes) = self.processes.lock() {
            processes.remove(id);
        }
    }
}

#[tauri::command]
pub fn list_downloads(manager: State<'_, DownloadManager>) -> Result<Vec<DownloadJob>, String> {
    manager
        .jobs
        .lock()
        .map(|jobs| jobs.clone())
        .map_err(|_| "다운로드 대기열을 읽지 못했습니다.".to_string())
}

#[tauri::command]
pub fn queue_download(
    app: AppHandle,
    manager: State<'_, DownloadManager>,
    url: String,
) -> Result<DownloadJob, String> {
    let url = validate_soundcloud_url(&url)?;
    let executable = find_ytdlp(&app).ok_or_else(|| {
        "yt-dlp.exe를 찾을 수 없습니다. 공식 실행 파일을 src-tauri/binaries/yt-dlp.exe에 배치해 주세요.".to_string()
    })?;
    let output_dir = app
        .path()
        .download_dir()
        .map_err(|error| format!("다운로드 폴더를 찾을 수 없습니다: {error}"))?
        .join("SoundCloud Downloader");
    std::fs::create_dir_all(&output_dir)
        .map_err(|error| format!("다운로드 폴더를 만들지 못했습니다: {error}"))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let id = format!("download-{timestamp}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let output_template = output_dir.join("%(artist,uploader)s - %(title)s.%(ext)s");

    // Pass every argument directly to yt-dlp (never through a shell) and pin
    // the operation to this single validated SoundCloud URL.
    let mut child = Command::new(executable)
        .args([
            "--newline",
            "--progress",
            "--progress-template",
            "download:%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s",
            "--no-colors",
            "--no-warnings",
            "--no-call-home",
            "--no-playlist",
            "--restrict-filenames",
            "--windows-filenames",
            "--output",
        ])
        .arg(output_template)
        .arg("--")
        .arg(&url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("yt-dlp를 시작하지 못했습니다: {error}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));
    let job = DownloadJob {
        id: id.clone(),
        url: url.clone(),
        title: url.clone(),
        status: DownloadStatus::Queued,
        progress: 0.0,
        speed: None,
        eta: None,
        output_path: Some(output_dir.to_string_lossy().into_owned()),
        error: None,
    };

    manager
        .jobs
        .lock()
        .map_err(|_| "다운로드 대기열을 사용할 수 없습니다.".to_string())?
        .push(job.clone());
    manager
        .processes
        .lock()
        .map_err(|_| "다운로드 프로세스를 등록하지 못했습니다.".to_string())?
        .insert(id.clone(), Arc::clone(&child));

    let manager = manager.inner().clone();
    let app_handle = app.clone();
    let job_id = id;
    thread::spawn(move || {
        manager.update(&app_handle, &job_id, |job| job.status = DownloadStatus::Downloading);
        let (sender, receiver) = mpsc::channel::<(bool, String)>();
        if let Some(stdout) = stdout {
            let sender = sender.clone();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().flatten() {
                    let _ = sender.send((false, line));
                }
            });
        }
        if let Some(stderr) = stderr {
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().flatten() {
                    let _ = sender.send((true, line));
                }
            });
        }
        drop(sender);

        let mut stderr_tail = String::new();
        let mut exit_status = None;
        loop {
            match receiver.recv_timeout(Duration::from_millis(150)) {
                Ok((is_stderr, line)) => {
                    if is_stderr {
                        if !stderr_tail.is_empty() {
                            stderr_tail.push('\n');
                        }
                        stderr_tail.push_str(&line);
                        if stderr_tail.chars().count() > 1_000 {
                            stderr_tail = stderr_tail.chars().rev().take(1_000).collect::<String>().chars().rev().collect();
                        }
                    } else if let Some((progress, speed, eta)) = parse_progress(&line) {
                        manager.update(&app_handle, &job_id, |job| {
                            job.progress = progress;
                            job.speed = speed;
                            job.eta = eta;
                        });
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if exit_status.is_some() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }

            if exit_status.is_none() {
                let status = child.lock().ok().and_then(|mut process| process.try_wait().ok().flatten());
                if status.is_some() {
                    exit_status = status;
                }
            }
            if exit_status.is_some() && receiver.try_recv().is_err() {
                // Reader threads close their pipes shortly after the process exits.
                if receiver.recv_timeout(Duration::from_millis(100)).is_err() {
                    break;
                }
            }
        }

        let cancelled = manager
            .jobs
            .lock()
            .ok()
            .and_then(|jobs| jobs.iter().find(|job| job.id == job_id).map(|job| job.status == DownloadStatus::Cancelled))
            .unwrap_or(false);
        manager.update(&app_handle, &job_id, |job| {
            if cancelled {
                job.status = DownloadStatus::Cancelled;
            } else if exit_status.as_ref().is_some_and(|status| status.success()) {
                job.status = DownloadStatus::Completed;
                job.progress = 100.0;
                job.speed = None;
                job.eta = None;
            } else {
                job.status = DownloadStatus::Failed;
                job.error = Some(if stderr_tail.trim().is_empty() {
                    "yt-dlp가 다운로드를 완료하지 못했습니다.".to_string()
                } else {
                    stderr_tail.trim().to_string()
                });
            }
        });
        manager.remove_process(&job_id);
    });

    Ok(job)
}

#[tauri::command]
pub fn cancel_download(manager: State<'_, DownloadManager>, id: String) -> Result<(), String> {
    let process = manager
        .processes
        .lock()
        .map_err(|_| "다운로드 프로세스 목록을 사용할 수 없습니다.".to_string())?
        .get(&id)
        .cloned()
        .ok_or_else(|| "실행 중인 다운로드를 찾을 수 없습니다.".to_string())?;
    process
        .lock()
        .map_err(|_| "다운로드 프로세스를 제어할 수 없습니다.".to_string())?
        .kill()
        .map_err(|error| format!("다운로드를 취소하지 못했습니다: {error}"))?;
    let mut jobs = manager
        .jobs
        .lock()
        .map_err(|_| "다운로드 대기열을 사용할 수 없습니다.".to_string())?;
    if let Some(job) = jobs.iter_mut().find(|job| job.id == id) {
        job.status = DownloadStatus::Cancelled;
    }
    Ok(())
}

fn parse_progress(line: &str) -> Option<(f32, Option<String>, Option<String>)> {
    let mut parts = line.split('|');
    let percent = parts.next()?.trim().trim_end_matches('%').trim().parse::<f32>().ok()?;
    let speed = parts.next().map(str::trim).filter(|value| !value.is_empty() && *value != "NA").map(str::to_owned);
    let eta = parts.next().map(str::trim).filter(|value| !value.is_empty() && *value != "NA").map(str::to_owned);
    Some((percent.clamp(0.0, 100.0), speed, eta))
}

#[cfg(test)]
mod tests {
    use super::{parse_progress, DownloadStatus};

    #[test]
    fn parses_ytdlp_progress_template() {
        let parsed = parse_progress(" 42.5%|1.2MiB/s|00:18").unwrap();
        assert_eq!(parsed.0, 42.5);
        assert_eq!(parsed.1.as_deref(), Some("1.2MiB/s"));
        assert_eq!(parsed.2.as_deref(), Some("00:18"));
    }

    #[test]
    fn progress_is_clamped_and_missing_fields_are_optional() {
        let parsed = parse_progress("120.0%|NA|NA").unwrap();
        assert_eq!(parsed.0, 100.0);
        assert!(parsed.1.is_none());
        assert!(parsed.2.is_none());
        assert_eq!(DownloadStatus::Queued, DownloadStatus::Queued);
    }
}
