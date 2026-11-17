# SoundCloud Downloader

Tauri 2 + React + TypeScript desktop app. The UI foundation is in place and the next milestone adds a Rust command for checking the yt-dlp executable and previewing SoundCloud metadata.

## Development

```sh
npm install
npm run tauri:dev
```

## Current milestone

- Rust/Tauri command registration and a minimal default capability
- yt-dlp runtime detection and version check
- SoundCloud HTTPS URL validation and metadata-only inspection (`--skip-download`)
- Playlist metadata preview capped at 200 entries
- The executable is intentionally not committed. For local testing, add the official Windows binary at `src-tauri/binaries/yt-dlp.exe`; the app reports when it is missing.
- No download queue, audio download, cookie authentication, or file management is implemented yet.

Rust URL-validation tests can be run with `cargo test --manifest-path src-tauri/Cargo.toml` in a configured Rust/Tauri environment. Build and runtime tests have not been run in this hosted editing session.
