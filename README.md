# SoundCloud Downloader

Tauri 2 + React + TypeScript 데스크톱 앱입니다. 사용자가 저장 권한을 가진 **공개 SoundCloud 트랙과 플레이리스트**를 분석하고, 선택한 트랙을 다운로드 대기열에 추가합니다.

## 현재 기능

- HTTPS `soundcloud.com` URL만 허용하고 yt-dlp로 공개 트랙/플레이리스트의 전체 트랙 메타데이터를 분석합니다.
- 큰 플레이리스트는 진행 상황을 표시하고 최대 20,000개까지 분석하며, 결과 목록은 100개씩 나눠 보여줍니다. 분석 중 취소할 수 있고 일부 항목을 가져오지 못하면 경고합니다.
- 트랙은 기본적으로 선택 해제되어 있습니다. 사용자가 선택한 트랙만 대기열에 추가됩니다.
- 플레이리스트에서 선택해 추가한 트랙은 다운로드 위치 아래에 플레이리스트 이름의 하위 폴더에 저장됩니다. 라이브러리는 이 하위 폴더까지 검색합니다. 개별 트랙 URL은 기존 다운로드 위치에 저장됩니다.
- 대기열은 `queued`, `downloading`, `completed`, `failed`, `cancelled` 상태를 저장하며 진행률·속도·남은 시간·오류를 표시합니다.
- 동시 다운로드는 1~8개로 설정할 수 있습니다. Rust 스케줄러가 다운로드마다 별도 작업 스레드와 yt-dlp 프로세스를 실행하며, 분할 스트림 병렬 처리는 작업 수에 따라 총 8개 이내로 조정합니다. 네트워크와 원본 스트림 종류에 따라 속도 향상 정도는 달라질 수 있습니다.
- 설정에서 한국어/영어 UI를 선택하고 About 창에서 앱 버전 및 저작권 정보를 확인할 수 있습니다.
- 개별/전체 취소와 실패·취소 작업 재시도를 지원합니다. 실행 중이던 작업은 앱을 다시 열면 중단 기록으로 복구됩니다.
- 대기 중인 작업 순서를 바꾸고, 여러 작업 기록을 한 번에 제거할 수 있습니다. 기록 제거는 내려받은 파일을 삭제하지 않습니다. 취소된 `.part` 파일은 보존하고 재시도 시 이어받기를 요청합니다(원본 서버가 지원하는 경우).
- 중복 트랙 처리 방식을 `건너뛰기`, `덮어쓰기`, `새 이름으로 저장` 중에서 설정할 수 있습니다. 기존 파일과 작업 기록의 트랙 ID를 기준으로 검사하며, 플레이리스트에서 이미 받은 트랙을 제외해 신규 트랙만 선택할 수 있습니다.
- 설정은 앱 데이터 폴더의 JSON 파일에 저장됩니다.
- 설정 백업/복원을 지원합니다. 백업에는 다운로드 폴더와 yt-dlp 실행 파일 경로가 포함되므로 공유할 때 주의하세요.
- 앱 버전, 엔진 버전/가용성, 작업 수 및 제한된 앱 이벤트 로그를 진단 보고서로 내보낼 수 있습니다. 보고서에는 트랙 URL이나 파일 경로를 기록하지 않습니다.
- 설정 화면에서 폴더 선택 대화상자로 다운로드 위치를 지정할 수 있으며, 사이드바에 yt-dlp와 FFmpeg의 실제 로드 상태를 표시합니다.
- 라이브러리는 설정된 다운로드 폴더의 오디오 파일을 검색합니다. 제목/아티스트 파일명 검색, 파일/폴더 열기, 확인 후 삭제를 지원하며 삭제 대상은 해당 폴더 내 오디오 파일로 제한됩니다.
- 화면과 통계는 실제 데이터만 표시합니다. 샘플 다운로드는 포함하지 않습니다.

## 외부 실행 파일

`yt-dlp`와 FFmpeg 실행 파일은 저장소나 설치 파일에 포함되지 않습니다. 앱은 설정에 지정한 yt-dlp 경로 또는 `PATH`에서 실행 파일을 찾고 버전을 확인합니다. 엔진이 없으면 해당 기능은 사용할 수 없으며, 앱이 이를 설치된 것처럼 표시하지 않습니다.

- **yt-dlp:** Python 런타임이 필요 없는 공식 standalone 실행 파일을 사용하세요. 공식 배포: [yt-dlp Releases](https://github.com/yt-dlp/yt-dlp/releases). 내려받은 버전의 배포 페이지에 있는 체크섬/서명 정보를 확인한 뒤 `PATH`에 추가하거나, 설정의 yt-dlp 경로에 실행 파일 전체 경로를 입력하세요. 비워 두면 `PATH`에서 검색합니다.
- **FFmpeg:** MP3 변환, 커버 아트 삽입, 메타데이터 삽입을 선택할 때 필요합니다. 공식 안내: [FFmpeg Download](https://ffmpeg.org/download.html). 운영체제에 맞는 빌드와 배포처의 검증 안내를 확인하고 `ffmpeg`를 `PATH`에 추가하세요.

원본 오디오를 선택하면 원본 스트림을 저장합니다. MP3 변환의 비트레이트 설정은 인코딩 설정일 뿐, 원본보다 높은 음질을 보장하지 않습니다.

## 개발 실행

Node.js LTS와 Rust stable이 필요합니다. Tauri의 운영체제별 개발 패키지도 설치해야 합니다.

```sh
npm ci
npm run tauri:dev
```

## Windows 설치 파일

`.github/workflows/windows-installer.yml`은 `windows-latest`에서 Node LTS, stable MSVC Rust와 NSIS를 이용해 빌드합니다. PR/`main` 푸시에서 테스트하고, workflow artifact에 `SC-dlp_Setup.exe`를 올립니다. `v*` 태그에서는 같은 파일을 GitHub Release에 첨부합니다.

Workflow는 `WINDOWS_SIGNING_PFX_BASE64` 및 `WINDOWS_SIGNING_PASSWORD` GitHub Actions secrets가 설정된 경우 앱 실행 파일과 설치 파일에 Authenticode 서명을 적용하고 확인합니다. 값이 없으면 기존처럼 unsigned 파일을 빌드하며, 릴리스에는 SHA-256 체크섬도 첨부합니다. 인증서는 신뢰할 수 있는 발급기관에서 별도로 취득하고, PFX와 암호를 저장소에 커밋하지 마세요. 코드 서명은 파일 출처와 무결성 확인을 돕지만 Smart App Control을 우회하거나 경고 제거를 보장하지 않습니다.

Tauri 자동 업데이트는 앱 시작 시 업데이트를 확인하고, 사용자가 동의한 경우에만 서명 검증된 NSIS 업데이트를 설치합니다. 업데이트 정보는 공개 GitHub Release의 `latest.json`에서 가져옵니다. GitHub 저장소의 **Settings → Secrets and variables → Actions**에서 `TAURI_SIGNING_PRIVATE_KEY`를 생성해 개인키 파일의 전체 내용을 secret으로 저장하세요. 키에 암호를 설정했다면 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`도 secret으로 설정해야 합니다. 개인키나 암호를 소스 코드, workflow, 채팅 또는 일반 Actions variables에 넣지 마세요. 현재 공개 릴리스 v0.1.5에는 업데이트 공개키가 포함되지 않았으므로, v0.1.6은 자동 업데이트가 아닌 최초 1회 수동 설치가 필요합니다. 이 bootstrap 버전부터 이후 버전은 서명이 확인되면 앱에서 업데이트할 수 있습니다. 개인키를 잃으면 기존 공개키를 포함한 설치본에 업데이트할 수 없으므로 안전하게 백업해야 합니다. v 태그로 릴리스하기 전에 `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`의 버전과 태그가 일치해야 합니다.

빌드 및 테스트 명령:

```sh
npm ci
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
```

## 사용 범위

브라우저 쿠키, 계정 인증, 비공개 콘텐츠 접근 또는 보호 조치 우회는 지원하지 않습니다. 앱은 공개 URL 분석만 허용하며, 사용자가 저장 권한을 가진 콘텐츠에만 사용해야 합니다.
