# SoundCloud Downloader

Tauri 2 + React + TypeScript desktop app.

## Current milestone

- Rust download queue commands: enqueue a validated SoundCloud track URL, inspect queue state, cancel an active process.
- yt-dlp runs as a direct subprocess (no shell), restricted to one URL at a time; original audio is saved under the user's Downloads/SoundCloud Downloader folder.
- Progress, speed, ETA, completion, cancellation and errors are emitted to the React queue view.
- Playlist screen lets the user queue individual tracks; Downloads page shows live status.
- Only download content you have permission to save. Cookies/authentication are not implemented.

## Requirements and running

```sh
npm install
npm run tauri:dev
```

The official `yt-dlp.exe` must be supplied at `src-tauri/binaries/yt-dlp.exe` for Windows local development. FFmpeg-based MP3 conversion, parallelism limits, persistent queue state, retry, and library management remain future work.

Rust tests can be run with `cargo test --manifest-path src-tauri/Cargo.toml` in a configured Tauri/Rust environment. Build and runtime tests were not run in this hosted editing session.
