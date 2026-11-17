# Bundled yt-dlp executable

The backend looks for the official Windows standalone executable at `src-tauri/binaries/yt-dlp.exe` during development and in the app resource directory after packaging. This binary is intentionally not committed here. Add the official `yt-dlp.exe` release asset through the project's approved dependency/binary distribution process before enabling metadata analysis in a packaged build.

The current CI/build configuration does not bundle an executable automatically; engine status reports unavailable until the binary is supplied.
