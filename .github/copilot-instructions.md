# SC-dlp project instructions

## Project overview

- This is a Tauri 2 desktop application for downloading and managing content from HTTPS SoundCloud URLs.
- The frontend is React 18, TypeScript, Vite, React Router, Tailwind CSS, and Tauri JavaScript APIs under `src/`.
- The native backend is Rust under `src-tauri/src/`; `lib.rs` contains the app state, Tauri commands, process management, persistence, and `yt-dlp` integration.
- Read [README.md](../README.md) for supported behavior and external-tool requirements.
- Preserve the app's public-content-only scope: do not add browser-cookie, account-authentication, private-content, or protection-bypass behavior.

## Architecture and boundaries

- Keep React page components and UI state in `src/App.tsx` unless the change clearly warrants extracted modules.
- Use Tauri commands for frontend-to-Rust operations and keep user-facing error strings understandable.
- When changing a command's request or response shape, update both the Rust `serde` struct/command and the matching TypeScript type and `invoke` call.
- For Tauri events, keep Rust event names and frontend `listen` event names and payload types in sync.
- Keep persistent state compatible with the JSON store. Preserve `serde(default)` and camel-case behavior where fields are serialized across the Tauri boundary.
- Keep process lifecycle, cancellation, queue scheduling, and shutdown handling in the Rust backend. Do not duplicate scheduler or worker state in React.
- Keep download paths, executable lookup, URL validation, and library file restrictions in Rust helpers rather than relying only on frontend checks.

## Frontend conventions

- Use strict TypeScript and existing patterns: function components, hooks, `async`/`await`, `invoke`, and Tauri event listeners.
- Reuse the existing card, empty-state, status-label, formatting, navigation, and Tailwind visual patterns.
- Keep UI copy in the app's existing Korean style unless the task explicitly requests localization changes.
- Use Tailwind utility classes and the theme colors in `tailwind.config.js`; avoid introducing a separate UI framework or ad hoc global CSS.
- Every async Tauri action must handle `unknown` failures and display a useful error, as existing pages do with `role="alert"`.
- Clean up Tauri event listeners and ignore stale async results after a component or analysis is no longer active.
- Do not invent sample jobs, library files, statistics, or engine availability. UI data must come from backend state or real files.

## Rust and external processes

- Validate all external or user-provided paths and URLs in Rust before using them.
- Only accept HTTPS `soundcloud.com` URLs and exact subdomains of `soundcloud.com`; never weaken URL validation.
- Keep `yt-dlp` and FFmpeg optional and discoverable from configuration or `PATH`. Never bundle or pretend either tool is available.
- Use `--ignore-config`, `--skip-download`, and `--dump-json` for analysis as appropriate to avoid downloading audio.
- Use the existing process-tree termination helpers for cancellations and shutdown; do not introduce direct shell invocations or unchecked process execution.
- Keep playlist output directories and library operations confined to the configured download root. Preserve filename sanitization and audio-extension restrictions.
- When changing settings, validate values before persistence and roll back in-memory state if JSON persistence fails.
- Preserve queue states and recovery behavior: `queued`, `downloading`, `completed`, `failed`, and `cancelled`.

## Testing and validation

- Add or update focused Rust tests in `src-tauri/src/lib.rs` for URL validation, settings, metadata parsing, cancellation or scheduling, filename safety, persistence, and library path confinement.
- Use the smallest relevant validation command while iterating:
  - Frontend and Tauri frontend build: `npm run build`
  - Rust formatting: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
  - Rust tests: `cargo test --manifest-path src-tauri/Cargo.toml`
  - Rust lint: `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings`
- For cross-layer or packaging changes, also run `npm run tauri:build` or the relevant Windows CI build steps.
- Do not add or remove dependencies without an explicit task need and a matching lockfile update.

## Change discipline

- Make surgical changes and preserve existing behavior unless the request explicitly changes it.
- Inspect nearby code and tests before editing. Reuse existing helpers and serialization patterns.
- Update directly related README documentation when commands, requirements, packaging, or supported behavior change.
