pub mod downloader;
mod queue;

use downloader::{engine_status, inspect_soundcloud_url};
use queue::{cancel_download, list_downloads, queue_download, DownloadManager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(DownloadManager::default())
        .invoke_handler(tauri::generate_handler![
            engine_status,
            inspect_soundcloud_url,
            queue_download,
            list_downloads,
            cancel_download
        ])
        .run(tauri::generate_context!())
        .expect("failed to run SoundCloud Downloader");
}
