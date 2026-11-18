# SoundCloud Downloader

Tauri 2 + React + TypeScript 데스크톱 앱입니다. 사용자가 저장 권한을 가진 **공개 SoundCloud 트랙과 플레이리스트**를 분석하고, 선택한 트랙을 다운로드 대기열에 추가합니다.

## 현재 기능

- HTTPS `soundcloud.com` URL만 허용하고 yt-dlp로 공개 메타데이터를 분석합니다.
- 트랙은 기본적으로 선택 해제되어 있습니다. 사용자가 선택한 트랙만 대기열에 추가됩니다.
- 대기열은 `queued`, `downloading`, `completed`, `failed`, `cancelled` 상태를 저장하며 진행률·속도·남은 시간·오류를 표시합니다.
- 개별/전체 취소와 실패·취소 작업 재시도를 지원합니다. 실행 중이던 작업은 앱을 다시 열면 중단 기록으로 복구됩니다.
- 설정은 앱 데이터 폴더의 JSON 파일에 저장됩니다.
- 라이브러리는 설정된 다운로드 폴더의 오디오 파일을 검색합니다. 제목/아티스트 파일명 검색, 파일/폴더 열기, 확인 후 삭제를 지원하며 삭제 대상은 해당 폴더 내 오디오 파일로 제한됩니다.
- 화면과 통계는 실제 데이터만 표시합니다. 샘플 다운로드는 포함하지 않습니다.

## 외부 실행 파일

`yt-dlp`와 FFmpeg 실행 파일은 저장소나 설치 파일에 포함되지 않습니다. 앱은 실행 시 `PATH`에서 파일을 찾고 `--version`으로 실제 상태를 표시합니다. 엔진이 없으면 해당 기능은 사용할 수 없으며, 앱이 이를 설치된 것처럼 표시하지 않습니다.

- **yt-dlp:** Python 런타임이 필요 없는 공식 standalone 실행 파일을 사용하세요. 공식 배포: [yt-dlp Releases](https://github.com/yt-dlp/yt-dlp/releases). 내려받은 버전의 배포 페이지에 있는 체크섬/서명 정보를 확인한 뒤 실행 파일을 `PATH`에 추가하세요.
- **FFmpeg:** MP3 변환, 커버 아트 삽입, 메타데이터 삽입을 선택할 때 필요합니다. 공식 안내: [FFmpeg Download](https://ffmpeg.org/download.html). 운영체제에 맞는 빌드와 배포처의 검증 안내를 확인하고 `ffmpeg`를 `PATH`에 추가하세요.

원본 오디오를 선택하면 원본 스트림을 저장합니다. MP3 변환의 비트레이트 설정은 인코딩 설정일 뿐, 원본보다 높은 음질을 보장하지 않습니다.

## 개발 실행

Node.js LTS와 Rust stable이 필요합니다. Tauri의 운영체제별 개발 패키지도 설치해야 합니다.

```sh
npm ci
npm run tauri:dev
```

## Windows 설치 파일

`.github/workflows/windows-installer.yml`은 `windows-latest`에서 Node LTS, stable MSVC Rust와 NSIS를 이용해 빌드합니다. PR/`main` 푸시에서 테스트하고, workflow artifact에 `SoundCloudDownloader_Setup.exe`를 올립니다. `v*` 태그에서는 같은 파일을 GitHub Release에 첨부합니다.

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
