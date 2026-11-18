import {
  Activity,
  ArrowDownToLine,
  ArrowUpRight,
  Disc3,
  FolderOpen,
  Home,
  Library,
  ListMusic,
  LoaderCircle,
  Play,
  RefreshCw,
  Search,
  Settings,
  Trash2,
  X,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Navigate, NavLink, Route, Routes, useLocation, useNavigate } from 'react-router-dom';

type Page = { title: string; subtitle: string; icon: LucideIcon };
type AppSettings = {
  downloadDirectory: string;
  audioFormat: 'original' | 'mp3';
  audioQuality: string;
  maxConcurrent: number;
  overwriteFiles: boolean;
  embedArtwork: boolean;
  embedMetadata: boolean;
};
type ToolStatus = { available: boolean; version: string | null; error: string | null };
type EngineStatus = { ytDlp: ToolStatus; ffmpeg: ToolStatus };
type TrackMetadata = { url: string; title: string; sourceId: string | null; artist: string | null; duration: number | null; selected: boolean };
type JobStatus = 'queued' | 'downloading' | 'completed' | 'failed' | 'cancelled';
type DownloadJob = {
  id: string;
  url: string;
  title: string;
  sourceId: string | null;
  artist: string | null;
  duration: number | null;
  status: JobStatus;
  progress: number;
  speed: string | null;
  eta: string | null;
  error: string | null;
  outputPath: string | null;
};
type LibraryFile = { path: string; name: string; size: number; modified: number | null };

function formatDuration(seconds: number | null) {
  if (seconds === null) return '길이 정보 없음';
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const statusLabels: Record<JobStatus, string> = {
  queued: '대기 중',
  downloading: '다운로드 중',
  completed: '완료',
  failed: '실패',
  cancelled: '취소됨',
};

const pages: Record<string, Page> = {
  '/': { title: '대시보드', subtitle: '다운로드와 라이브러리 상태를 확인하세요.', icon: Home },
  '/playlist': { title: '플레이리스트', subtitle: 'SoundCloud 링크에서 트랙 정보를 불러옵니다.', icon: ListMusic },
  '/downloads': { title: '다운로드', subtitle: '대기열과 작업 상태를 관리합니다.', icon: ArrowDownToLine },
  '/library': { title: '라이브러리', subtitle: '다운로드한 음악을 모아보고 관리합니다.', icon: Library },
  '/settings': { title: '설정', subtitle: '앱 환경과 다운로드 기본값을 설정합니다.', icon: Settings },
};

const nav = [
  { to: '/', label: '대시보드', icon: Home },
  { to: '/playlist', label: '플레이리스트', icon: ListMusic },
  { to: '/downloads', label: '다운로드', icon: ArrowDownToLine },
  { to: '/library', label: '라이브러리', icon: Library },
  { to: '/settings', label: '설정', icon: Settings },
];

function Card({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <section className={`rounded-2xl border border-white/[.07] bg-surface p-5 shadow-card ${className}`}>{children}</section>;
}

function EmptyState({ title, description }: { title: string; description: string }) {
  return (
    <Card className="flex min-h-64 flex-col items-center justify-center text-center">
      <div className="mb-4 rounded-2xl bg-brand/10 p-4 text-brand"><Disc3 size={24} /></div>
      <h2 className="text-lg font-semibold text-white">{title}</h2>
      <p className="mt-2 max-w-md text-sm leading-6 text-zinc-500">{description}</p>
    </Card>
  );
}

function Dashboard() {
  const [jobs, setJobs] = useState<DownloadJob[] | null>(null);
  const [libraryCount, setLibraryCount] = useState<number | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    invoke<DownloadJob[]>('get_download_jobs')
      .then((value) => { if (active) setJobs(value); })
      .catch((cause: unknown) => { if (active) setError(String(cause)); });
    invoke<LibraryFile[]>('get_library_files')
      .then((files) => { if (active) setLibraryCount(files.length); })
      .catch(() => { if (active) setLibraryCount(null); });
    return () => { active = false; };
  }, []);

  const counts = jobs ? [
    ['전체 작업', String(jobs.length), '실제 대기열 기록'],
    ['완료', String(jobs.filter((job) => job.status === 'completed').length), '완료된 작업'],
    ['진행/대기', String(jobs.filter((job) => job.status === 'queued' || job.status === 'downloading').length), '활성 작업'],
    ['라이브러리', libraryCount === null ? '—' : String(libraryCount), '오디오 파일'],
  ] : [];

  return (
    <div className="space-y-6">
      {error && <p role="alert" className="rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-300">{error}</p>}
      {jobs && (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          {counts.map(([label, value, note]) => (
            <Card key={label}>
              <p className="text-sm text-zinc-500">{label}</p>
              <p className="mt-3 text-3xl font-semibold tracking-tight text-white">{value}</p>
              <p className="mt-2 text-xs text-zinc-600">{note}</p>
            </Card>
          ))}
        </div>
      )}
      <Card>
        <div className="flex items-center gap-3">
          <div className="rounded-xl bg-brand/10 p-3 text-brand"><Activity size={20} /></div>
          <div>
            <h2 className="font-semibold text-white">권한이 있는 공개 콘텐츠</h2>
            <p className="text-sm text-zinc-500">HTTPS SoundCloud 트랙 또는 플레이리스트 URL을 분석합니다.</p>
          </div>
        </div>
        <p className="mt-5 text-sm leading-6 text-zinc-400">
          앱은 브라우저 쿠키나 비공개 접근을 사용하지 않습니다. 저장 권한이 있는 콘텐츠만 대기열에 추가하세요.
        </p>
        <NavLink
          to="/playlist"
          className="mt-5 inline-flex items-center gap-2 rounded-xl bg-brand px-4 py-2.5 text-sm font-semibold text-white transition hover:bg-orange-500"
        >
          플레이리스트 보기 <ArrowUpRight size={16} />
        </NavLink>
      </Card>
      {jobs && jobs.length === 0 && (
        <EmptyState title="다운로드 기록 없음" description="실제 작업이 추가되면 이곳에 표시됩니다. 샘플 트랙이나 임의의 통계는 표시하지 않습니다." />
      )}
    </div>
  );
}

function Playlist() {
  const navigate = useNavigate();
  const [url, setUrl] = useState('');
  const [tracks, setTracks] = useState<TrackMetadata[]>([]);
  const [loading, setLoading] = useState(false);
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const selectedCount = tracks.filter((track) => track.selected).length;

  async function analyze() {
    setLoading(true);
    setError('');
    setMessage('');
    setTracks([]);
    try {
      setTracks(await invoke<TrackMetadata[]>('analyze_url', { url }));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }

  async function addSelected() {
    setAdding(true);
    setError('');
    setMessage('');
    try {
      const jobs = await invoke<DownloadJob[]>('queue_downloads', { tracks });
      setMessage(`${jobs.length}개 작업을 대기열에 추가했습니다.`);
      navigate('/downloads');
    } catch (cause) {
      setError(String(cause));
    } finally {
      setAdding(false);
    }
  }

  function toggleAll(selected: boolean) {
    setTracks((current) => current.map((track) => ({ ...track, selected })));
  }

  return (
    <div className="space-y-5">
      <Card>
        <label htmlFor="sc-url" className="mb-3 block text-sm font-medium text-zinc-200">SoundCloud URL</label>
        <div className="flex flex-col gap-3 sm:flex-row">
          <input
            id="sc-url"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            onKeyDown={(event) => { if (event.key === 'Enter' && !loading) void analyze(); }}
            placeholder="https://soundcloud.com/..."
            className="h-12 min-w-0 flex-1 rounded-xl border border-white/10 bg-black/20 px-4 text-sm text-white placeholder:text-zinc-600"
          />
          <button type="button" onClick={analyze} disabled={!url.trim() || loading} className="inline-flex items-center justify-center gap-2 rounded-xl bg-brand px-5 py-3 text-sm font-semibold text-white disabled:opacity-50">
            {loading ? <LoaderCircle size={16} className="animate-spin" /> : <Search size={16} />}
            {loading ? '메타데이터 분석 중' : 'URL 분석'}
          </button>
        </div>
        <p className="mt-3 text-xs text-zinc-500">플레이리스트의 전체 트랙 정보를 분석합니다. 분석만으로 파일을 저장하지 않으며, 전체 선택 또는 원하는 트랙을 선택해 대기열에 추가하세요.</p>
      </Card>
      {error && <p role="alert" className="rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-300">{error}</p>}
      {message && <p role="status" className="rounded-xl border border-emerald-500/30 bg-emerald-500/10 p-4 text-sm text-emerald-300">{message}</p>}
      {tracks.length > 0 && (
        <Card>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h2 className="font-semibold text-white">분석 결과</h2>
              <p className="mt-1 text-sm text-zinc-500">{tracks.length}개 트랙 · 선택한 트랙만 큐에 추가됩니다.</p>
            </div>
            <div className="flex flex-wrap gap-2">
              <button type="button" onClick={() => toggleAll(true)} className="rounded-lg border border-white/10 px-3 py-2 text-xs text-zinc-300 hover:bg-white/5">전체 선택</button>
              <button type="button" onClick={() => toggleAll(false)} className="rounded-lg border border-white/10 px-3 py-2 text-xs text-zinc-300 hover:bg-white/5">전체 해제</button>
              <button type="button" onClick={addSelected} disabled={!selectedCount || adding} className="inline-flex items-center gap-2 rounded-lg bg-brand px-3 py-2 text-xs font-semibold text-white disabled:opacity-50">
                <Play size={14} /> 선택 항목 추가 ({selectedCount})
              </button>
            </div>
          </div>
          <div className="mt-4 divide-y divide-white/[.06]">
            {tracks.map((track, index) => (
              <label key={`${track.url}-${index}`} className="flex cursor-pointer items-center gap-3 py-3">
                <input
                  type="checkbox"
                  checked={track.selected}
                  onChange={(event) => setTracks((current) => current.map((item, itemIndex) => itemIndex === index ? { ...item, selected: event.target.checked } : item))}
                  className="h-4 w-4 accent-orange-500"
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium text-white">{track.title}</span>
                  <span className="block truncate text-xs text-zinc-500">{track.artist || '아티스트 정보 없음'}</span>
                </span>
                <span className="shrink-0 text-xs text-zinc-500">{formatDuration(track.duration)}</span>
              </label>
            ))}
          </div>
        </Card>
      )}
    </div>
  );
}

function Downloads() {
  const [jobs, setJobs] = useState<DownloadJob[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  async function refresh() {
    try {
      setJobs(await invoke<DownloadJob[]>('get_download_jobs'));
      setError('');
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
    let active = true;
    let unlisten: (() => void) | undefined;
    listen<DownloadJob>('download-progress', ({ payload }) => {
      if (active) {
        setJobs((current) => {
          const index = current.findIndex((job) => job.id === payload.id);
          if (index < 0) return [payload, ...current];
          return current.map((job) => job.id === payload.id ? payload : job);
        });
      }
    })
      .then((dispose) => { if (active) unlisten = dispose; else dispose(); })
      .catch((cause: unknown) => { if (active) setError(String(cause)); });
    return () => { active = false; unlisten?.(); };
  }, []);

  async function runAction(action: () => Promise<unknown>) {
    setError('');
    try {
      await action();
      await refresh();
    } catch (cause) {
      setError(String(cause));
    }
  }

  const activeJobs = jobs.filter((job) => job.status === 'queued' || job.status === 'downloading');

  return (
    <div className="space-y-5">
      {error && <p role="alert" className="whitespace-pre-line rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-300">{error}</p>}
      <Card>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 className="font-semibold text-white">다운로드 작업</h2>
            <p className="mt-1 text-sm text-zinc-500">{activeJobs.length}개 활성 · 작업 기록 {jobs.length}개</p>
          </div>
          <div className="flex gap-2">
            <button type="button" onClick={() => void refresh()} className="rounded-lg border border-white/10 p-2 text-zinc-300 hover:bg-white/5" aria-label="새로고침"><RefreshCw size={16} /></button>
            <button
              type="button"
              onClick={() => void runAction(() => invoke('cancel_all_downloads'))}
              disabled={!activeJobs.length}
              className="inline-flex items-center gap-2 rounded-lg border border-white/10 px-3 py-2 text-xs text-zinc-300 hover:bg-white/5 disabled:opacity-40"
            >
              <X size={14} /> 전체 취소
            </button>
          </div>
        </div>
      </Card>
      {loading && <EmptyState title="대기열을 불러오는 중" description="저장된 실제 작업 상태를 확인하고 있습니다." />}
      {!loading && jobs.length === 0 && <EmptyState title="대기열이 비어 있습니다" description="플레이리스트 페이지에서 URL을 분석하고 원하는 트랙을 선택하세요." />}
      {jobs.map((job) => (
        <Card key={job.id}>
          <div className="flex flex-wrap items-start gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="truncate text-sm font-semibold text-white">{job.title}</h3>
                <span className="rounded-full border border-white/10 px-2 py-0.5 text-xs text-zinc-400">{statusLabels[job.status]}</span>
              </div>
              <p className="mt-1 truncate text-xs text-zinc-500">{job.artist || '아티스트 정보 없음'} · {formatDuration(job.duration)}</p>
            </div>
            {job.status === 'queued' || job.status === 'downloading' ? (
              <button type="button" onClick={() => void runAction(() => invoke('cancel_download', { id: job.id }))} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label="다운로드 취소">
                <X size={16} />
              </button>
            ) : null}
            {job.status === 'failed' || job.status === 'cancelled' ? (
              <button type="button" onClick={() => void runAction(() => invoke('retry_download', { id: job.id }))} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label="다시 시도">
                <RefreshCw size={16} />
              </button>
            ) : null}
          </div>
          {job.status === 'downloading' && (
            <>
              <div className="mt-4 h-2 overflow-hidden rounded-full bg-white/10">
                <div className="h-full rounded-full bg-brand transition-all" style={{ width: `${job.progress}%` }} />
              </div>
              <p className="mt-2 text-xs text-zinc-500">{job.progress.toFixed(1)}% · {job.speed || '속도 확인 중'} · 남은 시간 {job.eta || '확인 중'}</p>
            </>
          )}
          {job.error && <p role="alert" className="mt-3 whitespace-pre-line text-xs leading-5 text-red-300">{job.error}</p>}
          {job.outputPath && <p className="mt-3 break-all text-xs text-emerald-400">{job.outputPath}</p>}
        </Card>
      ))}
    </div>
  );
}

function LibraryPage() {
  const [files, setFiles] = useState<LibraryFile[]>([]);
  const [query, setQuery] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);

  async function refresh() {
    try {
      setFiles(await invoke<LibraryFile[]>('get_library_files'));
      setError('');
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { void refresh(); }, []);

  async function openFolder() {
    try {
      await invoke('open_library_folder');
    } catch (cause) {
      setError(String(cause));
    }
  }

  async function deleteFile(file: LibraryFile) {
    if (!window.confirm(`"${file.name}" 파일을 삭제할까요? 이 작업은 되돌릴 수 없습니다.`)) return;
    try {
      await invoke('delete_library_file', { path: file.path });
      await refresh();
    } catch (cause) {
      setError(String(cause));
    }
  }

  const visibleFiles = files.filter((file) => file.name.toLowerCase().includes(query.trim().toLowerCase()));

  return (
    <div className="space-y-5">
      {error && <p role="alert" className="whitespace-pre-line rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-300">{error}</p>}
      <Card>
        <div className="flex flex-col gap-3 sm:flex-row">
          <label className="relative min-w-0 flex-1">
            <span className="sr-only">제목 또는 아티스트 검색</span>
            <Search size={16} className="absolute left-3 top-3.5 text-zinc-500" />
            <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="파일명(제목·아티스트) 검색" className="h-11 w-full rounded-xl border border-white/10 bg-black/20 pl-10 pr-3 text-sm text-white placeholder:text-zinc-600" />
          </label>
          <button type="button" onClick={() => void refresh()} className="rounded-xl border border-white/10 px-4 py-2 text-sm text-zinc-300 hover:bg-white/5">새로고침</button>
          <button type="button" onClick={() => void openFolder()} className="inline-flex items-center justify-center gap-2 rounded-xl border border-white/10 px-4 py-2 text-sm text-zinc-300 hover:bg-white/5">
            <FolderOpen size={16} /> 다운로드 폴더 열기
          </button>
        </div>
        <p className="mt-3 text-xs text-zinc-600">설정된 다운로드 폴더의 오디오 파일만 표시하고 삭제할 수 있습니다.</p>
      </Card>
      {loading && <EmptyState title="라이브러리를 불러오는 중" description="설정된 다운로드 폴더를 검색하고 있습니다." />}
      {!loading && visibleFiles.length === 0 && <EmptyState title="오디오 파일이 없습니다" description={query ? '검색어와 일치하는 파일이 없습니다.' : '완료된 다운로드가 설정된 폴더에 나타납니다.'} />}
      <div className="space-y-3">
        {visibleFiles.map((file) => (
          <Card key={file.path} className="flex flex-wrap items-center gap-3">
            <div className="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-brand/10 text-brand"><Disc3 size={19} /></div>
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium text-white">{file.name}</p>
              <p className="mt-1 text-xs text-zinc-500">
                {formatFileSize(file.size)}
                {file.modified ? ` · ${new Date(file.modified * 1000).toLocaleDateString()}` : ''}
              </p>
            </div>
            <button type="button" onClick={() => void invoke('open_library_file', { path: file.path }).catch((cause: unknown) => setError(String(cause)))} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label="파일 열기">
              <Play size={16} />
            </button>
            <button type="button" onClick={() => void openFolder()} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label="폴더 열기">
              <FolderOpen size={16} />
            </button>
            <button type="button" onClick={() => void deleteFile(file)} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-red-300" aria-label="파일 삭제">
              <Trash2 size={16} />
            </button>
          </Card>
        ))}
      </div>
    </div>
  );
}
function SettingsPage() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const tools: Array<{ name: string; status?: ToolStatus }> = [
    { name: 'yt-dlp', status: engine?.ytDlp },
    { name: 'FFmpeg', status: engine?.ffmpeg },
  ];

  useEffect(() => {
    Promise.all([
      invoke<AppSettings>('get_settings'),
      invoke<EngineStatus>('get_engine_status'),
    ])
      .then(([loadedSettings, loadedEngine]) => {
        setSettings(loadedSettings);
        setEngine(loadedEngine);
      })
      .catch((cause: unknown) => setError(String(cause)));
  }, []);

  function update<K extends keyof AppSettings>(key: K, value: AppSettings[K]) {
    setSettings((current) => current ? { ...current, [key]: value } : current);
    setSaved(false);
  }

  async function save() {
    if (!settings) return;
    setSaving(true);
    setSaved(false);
    setError('');
    try {
      setSettings(await invoke<AppSettings>('save_settings', { settings }));
      setSaved(true);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSaving(false);
    }
  }

  if (!settings && !error) {
    return <EmptyState title="설정을 불러오는 중" description="저장된 앱 설정을 확인하고 있습니다." />;
  }

  return (
    <div className="space-y-5">
      {error && <p role="alert" className="rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-300">{error}</p>}
      {settings && (
        <>
          <Card>
            <h2 className="font-semibold text-white">다운로드 기본값</h2>
            <div className="mt-5 grid gap-5 sm:grid-cols-2">
              <label className="text-sm text-zinc-300 sm:col-span-2">
                다운로드 폴더
                <input
                  value={settings.downloadDirectory}
                  onChange={(event) => update('downloadDirectory', event.target.value)}
                  className="mt-2 h-11 w-full rounded-xl border border-white/10 bg-black/20 px-3 text-white"
                />
              </label>
              <label className="text-sm text-zinc-300">
                저장 형식
                <select
                  value={settings.audioFormat}
                  onChange={(event) => update('audioFormat', event.target.value as AppSettings['audioFormat'])}
                  className="mt-2 h-11 w-full rounded-xl border border-white/10 bg-[#15151c] px-3 text-white"
                >
                  <option value="original">원본 오디오</option>
                  <option value="mp3">MP3 변환</option>
                </select>
              </label>
              <label className="text-sm text-zinc-300">
                MP3 음질
                <select
                  value={settings.audioQuality}
                  onChange={(event) => update('audioQuality', event.target.value)}
                  disabled={settings.audioFormat !== 'mp3'}
                  className="mt-2 h-11 w-full rounded-xl border border-white/10 bg-[#15151c] px-3 text-white disabled:opacity-50"
                >
                  {['128', '192', '256', '320'].map((quality) => (
                    <option key={quality} value={quality}>{quality} kbps</option>
                  ))}
                </select>
                <span className="mt-1 block text-xs text-zinc-600">원본 스트림보다 높은 음질을 보장하지 않습니다.</span>
              </label>
              <label className="text-sm text-zinc-300">
                동시 작업 수
                <select
                  value={settings.maxConcurrent}
                  onChange={(event) => update('maxConcurrent', Number(event.target.value))}
                  className="mt-2 h-11 w-full rounded-xl border border-white/10 bg-[#15151c] px-3 text-white"
                >
                  {[1, 2, 3, 4].map((count) => <option key={count} value={count}>{count}개</option>)}
                </select>
              </label>
              <p className="text-xs leading-5 text-zinc-500 sm:col-span-2">
                MP3 변환과 커버 아트/메타데이터 삽입에는 FFmpeg가 필요합니다. 원본 오디오를 선택하면 변환하지 않습니다.
              </p>
              <div className="space-y-3 sm:col-span-2">
                {([
                  ['overwriteFiles', '같은 이름의 파일 덮어쓰기'],
                  ['embedArtwork', '커버 아트 보존'],
                  ['embedMetadata', '메타데이터 보존'],
                ] as const).map(([key, label]) => (
                  <label key={key} className="flex items-center gap-3 text-sm text-zinc-300">
                    <input
                      type="checkbox"
                      checked={settings[key]}
                      onChange={(event) => update(key, event.target.checked)}
                      className="h-4 w-4 accent-orange-500"
                    />
                    {label}
                  </label>
                ))}
              </div>
            </div>
            <div className="mt-6 flex flex-wrap items-center gap-3">
              <button
                type="button"
                onClick={save}
                disabled={saving}
                className="rounded-xl bg-brand px-4 py-2.5 text-sm font-semibold text-white disabled:opacity-50"
              >
                {saving ? '저장 중…' : '설정 저장'}
              </button>
              {saved && <span role="status" className="text-sm text-emerald-400">설정을 저장했습니다.</span>}
            </div>
          </Card>
          <Card>
            <h2 className="font-semibold text-white">다운로드 엔진</h2>
            <p className="mt-2 text-sm text-zinc-500">실행 파일은 저장소에 포함되지 않습니다. 설치 후 PATH에서 확인합니다.</p>
            <div className="mt-4 grid gap-3 sm:grid-cols-2">
              {tools.map(({ name, status: tool }) => (
                <div key={name} className="rounded-xl border border-white/[.07] bg-black/20 p-4">
                  <p className="text-sm font-medium text-white">{name}</p>
                  <p className={`mt-2 text-sm ${tool?.available ? 'text-emerald-400' : 'text-amber-400'}`}>
                    {tool ? tool.available ? tool.version : tool.error : '확인 중'}
                  </p>
                </div>
              ))}
            </div>
          </Card>
        </>
      )}
    </div>
  );
}

export default function App() {
  const location = useLocation();
  const navigate = useNavigate();
  const page = pages[location.pathname] ?? pages['/'];
  const Icon = page.icon;

  return (
    <div className="min-h-screen md:flex">
      <aside className="flex shrink-0 flex-col border-b border-white/[.07] bg-[#0e0e14] md:sticky md:top-0 md:h-screen md:w-64 md:border-b-0 md:border-r">
        <div className="flex items-center gap-3 px-5 py-5">
          <div className="grid h-10 w-10 place-items-center rounded-xl bg-brand text-white"><Disc3 size={22} /></div>
          <div><p className="font-semibold tracking-tight text-white">SoundCloud</p><p className="text-xs text-zinc-500">Downloader</p></div>
        </div>
        <nav className="flex gap-1 overflow-x-auto px-3 pb-3 md:block md:space-y-1 md:px-3 md:py-5">
          {nav.map(({ to, label, icon: NavIcon }) => (
            <NavLink
              key={to}
              to={to}
              end={to === '/'}
              className={({ isActive }) => `flex shrink-0 items-center gap-3 rounded-xl px-3 py-2.5 text-sm transition ${isActive ? 'bg-white/[.08] text-white' : 'text-zinc-500 hover:bg-white/[.04] hover:text-zinc-200'}`}
            >
              <NavIcon size={17} /><span>{label}</span>
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto hidden p-4 md:block">
          <div className="rounded-xl border border-white/[.07] bg-white/[.025] p-3">
            <div className="flex items-center gap-2 text-xs font-medium text-zinc-400">
              <span className="h-2 w-2 rounded-full bg-brand" /> 엔진 정보
            </div>
            <p className="mt-2 text-xs leading-5 text-zinc-600">yt-dlp 및 FFmpeg 상태는 설정에서 확인하세요.</p>
          </div>
        </div>
      </aside>
      <div className="min-w-0 flex-1">
        <header className="sticky top-0 z-10 flex h-[73px] items-center justify-between border-b border-white/[.07] bg-canvas/90 px-5 backdrop-blur-xl md:px-8">
          <div className="flex min-w-0 items-center gap-3">
            <div className="grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-white/[.05] text-zinc-300"><Icon size={17} /></div>
            <div className="min-w-0"><h1 className="truncate text-sm font-semibold text-white">{page.title}</h1><p className="hidden truncate text-xs text-zinc-600 sm:block">{page.subtitle}</p></div>
          </div>
          <button
            type="button"
            onClick={() => navigate('/settings')}
            className="ml-3 shrink-0 rounded-xl border border-white/10 p-2.5 text-zinc-400 transition hover:bg-white/[.05] hover:text-white"
            aria-label="설정으로 이동"
          >
            <Settings size={17} />
          </button>
        </header>
        <main className="mx-auto max-w-[1440px] p-4 sm:p-5 md:p-8">
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/playlist" element={<Playlist />} />
            <Route path="/downloads" element={<Downloads />} />
            <Route path="/library" element={<LibraryPage />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
          <footer className="mt-8 border-t border-white/[.06] pt-4 text-xs text-zinc-700">
            SoundCloud Downloader · 저장 권한이 있는 공개 콘텐츠만
          </footer>
        </main>
      </div>
    </div>
  );
}
