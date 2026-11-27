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
import { createContext, useCallback, useContext, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getVersion } from '@tauri-apps/api/app';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Navigate, NavLink, Route, Routes, useLocation, useNavigate } from 'react-router-dom';

type Language = 'ko' | 'en';
type Page = { title: string; subtitle: string; icon: LucideIcon };
type AppSettings = {
  downloadDirectory: string;
  ytDlpPath: string;
  audioFormat: 'original' | 'mp3';
  audioQuality: string;
  maxConcurrent: number;
  language: Language;
  overwriteFiles: boolean;
  embedArtwork: boolean;
  embedMetadata: boolean;
};
type ToolStatus = { available: boolean; version: string | null; error: string | null };
type EngineStatus = { ytDlp: ToolStatus; ffmpeg: ToolStatus };
type TrackMetadata = { url: string; title: string; sourceId: string | null; artist: string | null; duration: number | null; selected: boolean };
type PlaylistAnalysis = { tracks: TrackMetadata[]; playlistName: string | null; totalEntries: number; skippedEntries: number; warning: string | null };
type AnalysisProgress = { id: string; processed: number; total: number | null };
type JobStatus = 'queued' | 'downloading' | 'completed' | 'failed' | 'cancelled';
type DownloadJob = {
  id: string;
  url: string;
  title: string;
  playlistName: string | null;
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

const LanguageContext = createContext<Language>('ko');

function useText() {
  const language = useContext(LanguageContext);
  return (korean: string, english: string) => language === 'ko' ? korean : english;
}

function formatDuration(seconds: number | null, language: Language) {
  if (seconds === null) return language === 'ko' ? '길이 정보 없음' : 'Duration unavailable';
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const statusLabels: Record<JobStatus, { ko: string; en: string }> = {
  queued: { ko: '대기 중', en: 'Queued' },
  downloading: { ko: '다운로드 중', en: 'Downloading' },
  completed: { ko: '완료', en: 'Completed' },
  failed: { ko: '실패', en: 'Failed' },
  cancelled: { ko: '취소됨', en: 'Cancelled' },
};
const ANALYSIS_PAGE_SIZE = 100;

const pages: Record<string, { ko: Page; en: Page }> = {
  '/': {
    ko: { title: '대시보드', subtitle: '다운로드와 라이브러리 상태를 확인하세요.', icon: Home },
    en: { title: 'Dashboard', subtitle: 'Overview of downloads and your library.', icon: Home },
  },
  '/playlist': {
    ko: { title: '플레이리스트', subtitle: 'SoundCloud 링크에서 트랙 정보를 불러옵니다.', icon: ListMusic },
    en: { title: 'Playlist', subtitle: 'Load track information from a SoundCloud link.', icon: ListMusic },
  },
  '/downloads': {
    ko: { title: '다운로드', subtitle: '대기열과 작업 상태를 관리합니다.', icon: ArrowDownToLine },
    en: { title: 'Downloads', subtitle: 'Manage the queue and download status.', icon: ArrowDownToLine },
  },
  '/library': {
    ko: { title: '라이브러리', subtitle: '다운로드한 음악을 모아보고 관리합니다.', icon: Library },
    en: { title: 'Library', subtitle: 'Browse and manage downloaded audio.', icon: Library },
  },
  '/settings': {
    ko: { title: '설정', subtitle: '앱 환경과 다운로드 기본값을 설정합니다.', icon: Settings },
    en: { title: 'Settings', subtitle: 'Configure app preferences and download defaults.', icon: Settings },
  },
};

const nav = [
  { to: '/', ko: '대시보드', en: 'Dashboard', icon: Home },
  { to: '/playlist', ko: '플레이리스트', en: 'Playlist', icon: ListMusic },
  { to: '/downloads', ko: '다운로드', en: 'Downloads', icon: ArrowDownToLine },
  { to: '/library', ko: '라이브러리', en: 'Library', icon: Library },
  { to: '/settings', ko: '설정', en: 'Settings', icon: Settings },
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
  const t = useText();
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
    [t('전체 작업', 'Total jobs'), String(jobs.length), t('실제 대기열 기록', 'Actual queue records')],
    [t('완료', 'Completed'), String(jobs.filter((job) => job.status === 'completed').length), t('완료된 작업', 'Completed jobs')],
    [t('진행/대기', 'Active/queued'), String(jobs.filter((job) => job.status === 'queued' || job.status === 'downloading').length), t('활성 작업', 'Active jobs')],
    [t('라이브러리', 'Library'), libraryCount === null ? '—' : String(libraryCount), t('오디오 파일', 'Audio files')],
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
            <h2 className="font-semibold text-white">{t('권한이 있는 공개 콘텐츠', 'Public content you are authorized to save')}</h2>
            <p className="text-sm text-zinc-500">{t('HTTPS SoundCloud 트랙 또는 플레이리스트 URL을 분석합니다.', 'Analyze an HTTPS SoundCloud track or playlist URL.')}</p>
          </div>
        </div>
        <p className="mt-5 text-sm leading-6 text-zinc-400">
          {t('앱은 브라우저 쿠키나 비공개 접근을 사용하지 않습니다. 저장 권한이 있는 콘텐츠만 대기열에 추가하세요.', 'The app does not use browser cookies or private access. Only queue content you are authorized to save.')}
        </p>
        <NavLink
          to="/playlist"
          className="mt-5 inline-flex items-center gap-2 rounded-xl bg-brand px-4 py-2.5 text-sm font-semibold text-white transition hover:bg-orange-500"
        >
          {t('플레이리스트 보기', 'Open playlist')} <ArrowUpRight size={16} />
        </NavLink>
      </Card>
      {jobs && jobs.length === 0 && (
        <EmptyState title={t('다운로드 기록 없음', 'No download history')} description={t('실제 작업이 추가되면 이곳에 표시됩니다. 샘플 트랙이나 임의의 통계는 표시하지 않습니다.', 'Real jobs will appear here. No sample tracks or fabricated statistics are shown.')} />
      )}
    </div>
  );
}

function Playlist() {
  const t = useText();
  const language = useContext(LanguageContext);
  const navigate = useNavigate();
  const [url, setUrl] = useState('');
  const [tracks, setTracks] = useState<TrackMetadata[]>([]);
  const [playlistName, setPlaylistName] = useState<string | null>(null);
  const [analysisSummary, setAnalysisSummary] = useState<{ totalEntries: number; skippedEntries: number; warning: string | null } | null>(null);
  const [loading, setLoading] = useState(false);
  const [adding, setAdding] = useState(false);
  const [analysisId, setAnalysisId] = useState<string | null>(null);
  const [analysisProgress, setAnalysisProgress] = useState<AnalysisProgress | null>(null);
  const [cancelingAnalysis, setCancelingAnalysis] = useState(false);
  const [page, setPage] = useState(0);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const cancelRequested = useRef(false);
  const selectedCount = tracks.filter((track) => track.selected).length;
  const pageCount = Math.max(1, Math.ceil(tracks.length / ANALYSIS_PAGE_SIZE));
  const pageTracks = tracks.slice(page * ANALYSIS_PAGE_SIZE, (page + 1) * ANALYSIS_PAGE_SIZE);

  async function analyze() {
    setLoading(true);
    setError('');
    setMessage('');
    setTracks([]);
    setPlaylistName(null);
    setAnalysisSummary(null);
    setPage(0);
    setAnalysisProgress(null);
    setCancelingAnalysis(false);
    cancelRequested.current = false;
    const id = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    setAnalysisId(id);
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await listen<AnalysisProgress>('playlist-analysis-progress', ({ payload }) => {
        if (payload.id === id) setAnalysisProgress(payload);
      });
      const result = await invoke<PlaylistAnalysis>('analyze_url', { id, url });
      setTracks(result.tracks);
      setPlaylistName(result.playlistName);
      setAnalysisSummary({ totalEntries: result.totalEntries, skippedEntries: result.skippedEntries, warning: result.warning });
    } catch (cause) {
      if (cancelRequested.current) setMessage(t('플레이리스트 분석을 취소했습니다.', 'Playlist analysis was cancelled.'));
      else setError(String(cause));
    } finally {
      unlisten?.();
      setLoading(false);
      setAnalysisId(null);
      setAnalysisProgress(null);
      setCancelingAnalysis(false);
    }
  }

  async function cancelAnalysis() {
    if (!analysisId) return;
    cancelRequested.current = true;
    setCancelingAnalysis(true);
    try {
      await invoke('cancel_playlist_analysis', { id: analysisId });
    } catch (cause) {
      cancelRequested.current = false;
      setCancelingAnalysis(false);
      setError(String(cause));
    }
  }

  async function addSelected() {
    setAdding(true);
    setError('');
    setMessage('');
    try {
      const jobs = await invoke<DownloadJob[]>('queue_downloads', { tracks, playlistName });
      setMessage(t(`${jobs.length}개 작업을 대기열에 추가했습니다.`, `Added ${jobs.length} jobs to the queue.`));
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
            {loading ? t('메타데이터 분석 중', 'Analyzing metadata') : t('URL 분석', 'Analyze URL')}
          </button>
          {loading && (
            <button type="button" onClick={() => void cancelAnalysis()} disabled={cancelingAnalysis} className="inline-flex items-center justify-center gap-2 rounded-xl border border-white/10 px-4 py-3 text-sm text-zinc-300 disabled:opacity-50">
              <X size={16} /> {cancelingAnalysis ? t('취소 중', 'Cancelling...') : t('분석 취소', 'Cancel analysis')}
            </button>
          )}
        </div>
        <p className="mt-3 text-xs text-zinc-500">{t('최대 20,000개 트랙의 정보를 분석해 100개씩 표시합니다. 분석만으로 파일을 저장하지 않으며, 전체 선택 또는 원하는 트랙을 선택해 대기열에 추가하세요.', 'Analyze up to 20,000 tracks and show 100 at a time. Analysis does not download files; select all or choose tracks to add to the queue.')}</p>
        {loading && (
          <div className="mt-4" role="status" aria-live="polite">
            <p className="text-sm text-zinc-300">
              {analysisProgress
                ? t(`트랙 정보 ${analysisProgress.processed}${analysisProgress.total ? ` / ${analysisProgress.total}` : ''}개 확인 중`, `Checking track information: ${analysisProgress.processed}${analysisProgress.total ? ` / ${analysisProgress.total}` : ''}`)
                : t('SoundCloud에서 트랙 정보를 가져오는 중', 'Loading track information from SoundCloud')}
            </p>
            {analysisProgress?.total ? (
              <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-white/10">
                <div className="h-full rounded-full bg-brand transition-all" style={{ width: `${Math.min(100, analysisProgress.processed / analysisProgress.total * 100)}%` }} />
              </div>
            ) : null}
          </div>
        )}
      </Card>
      {error && <p role="alert" className="rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-300">{error}</p>}
      {message && <p role="status" className="rounded-xl border border-emerald-500/30 bg-emerald-500/10 p-4 text-sm text-emerald-300">{message}</p>}
      {tracks.length > 0 && (
        <Card>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h2 className="font-semibold text-white">{t('분석 결과', 'Analysis results')}</h2>
              <p className="mt-1 text-sm text-zinc-500">
                {t(`전체 ${analysisSummary?.totalEntries ?? tracks.length}개 중 ${tracks.length}개 확인 · 선택한 트랙만 큐에 추가됩니다.`, `${tracks.length} of ${analysisSummary?.totalEntries ?? tracks.length} entries found · only selected tracks are queued.`)}
              </p>
              {playlistName && <p className="mt-1 text-xs text-zinc-500">{t('다운로드 폴더:', 'Download folder:')} {playlistName}</p>}
            </div>
            <div className="flex flex-wrap gap-2">
              <button type="button" onClick={() => toggleAll(true)} className="rounded-lg border border-white/10 px-3 py-2 text-xs text-zinc-300 hover:bg-white/5">{t('전체 선택', 'Select all')}</button>
              <button type="button" onClick={() => toggleAll(false)} className="rounded-lg border border-white/10 px-3 py-2 text-xs text-zinc-300 hover:bg-white/5">{t('전체 해제', 'Deselect all')}</button>
              <button type="button" onClick={addSelected} disabled={!selectedCount || adding} className="inline-flex items-center gap-2 rounded-lg bg-brand px-3 py-2 text-xs font-semibold text-white disabled:opacity-50">
                <Play size={14} /> {t(`선택 항목 추가 (${selectedCount})`, `Add selected (${selectedCount})`)}
              </button>
            </div>
          </div>
          {analysisSummary?.skippedEntries || analysisSummary?.warning ? (
            <p role="alert" className="mt-3 rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-300">
              {analysisSummary.warning || t(`${analysisSummary.skippedEntries}개 항목은 트랙 정보를 가져오지 못해 제외되었습니다.`, `${analysisSummary.skippedEntries} entries could not be loaded and were skipped.`)}
            </p>
          ) : null}
          {pageCount > 1 && (
            <div className="mt-4 flex items-center justify-between border-t border-white/[.06] pt-3">
              <p className="text-xs text-zinc-500">{t(`${tracks.length}개 중 ${page * ANALYSIS_PAGE_SIZE + 1}–${Math.min((page + 1) * ANALYSIS_PAGE_SIZE, tracks.length)}개 표시`, `Showing ${page * ANALYSIS_PAGE_SIZE + 1}–${Math.min((page + 1) * ANALYSIS_PAGE_SIZE, tracks.length)} of ${tracks.length}`)}</p>
              <div className="flex items-center gap-3">
                <button type="button" onClick={() => setPage((current) => Math.max(0, current - 1))} disabled={page === 0} className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-zinc-300 disabled:opacity-40">{t('이전', 'Previous')}</button>
                <span className="text-xs text-zinc-500">{page + 1} / {pageCount}</span>
                <button type="button" onClick={() => setPage((current) => Math.min(pageCount - 1, current + 1))} disabled={page + 1 >= pageCount} className="rounded-lg border border-white/10 px-3 py-1.5 text-xs text-zinc-300 disabled:opacity-40">{t('다음', 'Next')}</button>
              </div>
            </div>
          )}
          <div className="mt-4 divide-y divide-white/[.06]">
            {pageTracks.map((track, pageIndex) => {
              const index = page * ANALYSIS_PAGE_SIZE + pageIndex;
              return <label key={`${track.url}-${index}`} className="flex cursor-pointer items-center gap-3 py-3">
                <input
                  type="checkbox"
                  checked={track.selected}
                  onChange={(event) => setTracks((current) => current.map((item, itemIndex) => itemIndex === index ? { ...item, selected: event.target.checked } : item))}
                  className="h-4 w-4 accent-orange-500"
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium text-white">{track.title}</span>
                  <span className="block truncate text-xs text-zinc-500">{track.artist || t('아티스트 정보 없음', 'Artist unavailable')}</span>
                </span>
                <span className="shrink-0 text-xs text-zinc-500">{formatDuration(track.duration, language)}</span>
              </label>;
            })}
          </div>
        </Card>
      )}
    </div>
  );
}

function Downloads() {
  const t = useText();
  const language = useContext(LanguageContext);
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
            <h2 className="font-semibold text-white">{t('다운로드 작업', 'Download jobs')}</h2>
            <p className="mt-1 text-sm text-zinc-500">{t(`${activeJobs.length}개 활성 · 작업 기록 ${jobs.length}개`, `${activeJobs.length} active · ${jobs.length} jobs recorded`)}</p>
          </div>
          <div className="flex gap-2">
            <button type="button" onClick={() => void refresh()} className="rounded-lg border border-white/10 p-2 text-zinc-300 hover:bg-white/5" aria-label={t('새로고침', 'Refresh')}><RefreshCw size={16} /></button>
            <button
              type="button"
              onClick={() => void runAction(() => invoke('cancel_all_downloads'))}
              disabled={!activeJobs.length}
              className="inline-flex items-center gap-2 rounded-lg border border-white/10 px-3 py-2 text-xs text-zinc-300 hover:bg-white/5 disabled:opacity-40"
            >
              <X size={14} /> {t('전체 취소', 'Cancel all')}
            </button>
          </div>
        </div>
      </Card>
      {loading && <EmptyState title={t('대기열을 불러오는 중', 'Loading queue')} description={t('저장된 실제 작업 상태를 확인하고 있습니다.', 'Checking saved job status.')} />}
      {!loading && jobs.length === 0 && <EmptyState title={t('대기열이 비어 있습니다', 'Queue is empty')} description={t('플레이리스트 페이지에서 URL을 분석하고 원하는 트랙을 선택하세요.', 'Analyze a URL on the Playlist page and select tracks to download.')} />}
      {jobs.map((job) => (
        <Card key={job.id}>
          <div className="flex flex-wrap items-start gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <h3 className="truncate text-sm font-semibold text-white">{job.title}</h3>
                <span className="rounded-full border border-white/10 px-2 py-0.5 text-xs text-zinc-400">{statusLabels[job.status][language]}</span>
              </div>
              <p className="mt-1 truncate text-xs text-zinc-500">{job.artist || t('아티스트 정보 없음', 'Artist unavailable')} · {formatDuration(job.duration, language)}</p>
            </div>
            {job.status === 'queued' || job.status === 'downloading' ? (
              <button type="button" onClick={() => void runAction(() => invoke('cancel_download', { id: job.id }))} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label={t('다운로드 취소', 'Cancel download')}>
                <X size={16} />
              </button>
            ) : null}
            {job.status === 'failed' || job.status === 'cancelled' ? (
              <button type="button" onClick={() => void runAction(() => invoke('retry_download', { id: job.id }))} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label={t('다시 시도', 'Retry download')}>
                <RefreshCw size={16} />
              </button>
            ) : null}
          </div>
          {job.status === 'downloading' && (
            <>
              <div className="mt-4 h-2 overflow-hidden rounded-full bg-white/10">
                <div className="h-full rounded-full bg-brand transition-all" style={{ width: `${job.progress}%` }} />
              </div>
              <p className="mt-2 text-xs text-zinc-500">{job.progress.toFixed(1)}% · {job.speed || t('속도 확인 중', 'Checking speed')} · {t('남은 시간', 'ETA')} {job.eta || t('확인 중', 'Checking')}</p>
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
  const t = useText();
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
    if (!window.confirm(t(`"${file.name}" 파일을 삭제할까요? 이 작업은 되돌릴 수 없습니다.`, `Delete "${file.name}"? This action cannot be undone.`))) return;
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
            <span className="sr-only">{t('제목 또는 아티스트 검색', 'Search title or artist')}</span>
            <Search size={16} className="absolute left-3 top-3.5 text-zinc-500" />
            <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t('파일명(제목·아티스트) 검색', 'Search filename (title or artist)')} className="h-11 w-full rounded-xl border border-white/10 bg-black/20 pl-10 pr-3 text-sm text-white placeholder:text-zinc-600" />
          </label>
          <button type="button" onClick={() => void refresh()} className="rounded-xl border border-white/10 px-4 py-2 text-sm text-zinc-300 hover:bg-white/5">{t('새로고침', 'Refresh')}</button>
          <button type="button" onClick={() => void openFolder()} className="inline-flex items-center justify-center gap-2 rounded-xl border border-white/10 px-4 py-2 text-sm text-zinc-300 hover:bg-white/5">
            <FolderOpen size={16} /> {t('다운로드 폴더 열기', 'Open download folder')}
          </button>
        </div>
        <p className="mt-3 text-xs text-zinc-600">{t('설정된 다운로드 폴더의 오디오 파일만 표시하고 삭제할 수 있습니다.', 'Only audio files in the configured download folder can be viewed and deleted.')}</p>
      </Card>
      {loading && <EmptyState title={t('라이브러리를 불러오는 중', 'Loading library')} description={t('설정된 다운로드 폴더를 검색하고 있습니다.', 'Searching the configured download folder.')} />}
      {!loading && visibleFiles.length === 0 && <EmptyState title={t('오디오 파일이 없습니다', 'No audio files')} description={query ? t('검색어와 일치하는 파일이 없습니다.', 'No files match your search.') : t('완료된 다운로드가 설정된 폴더에 나타납니다.', 'Completed downloads will appear in the configured folder.')} />}
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
            <button type="button" onClick={() => void invoke('open_library_file', { path: file.path }).catch((cause: unknown) => setError(String(cause)))} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label={t('파일 열기', 'Open file')}>
              <Play size={16} />
            </button>
            <button type="button" onClick={() => void openFolder()} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label={t('폴더 열기', 'Open folder')}>
              <FolderOpen size={16} />
            </button>
            <button type="button" onClick={() => void deleteFile(file)} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-red-300" aria-label={t('파일 삭제', 'Delete file')}>
              <Trash2 size={16} />
            </button>
          </Card>
        ))}
      </div>
    </div>
  );
}
function SettingsPage({
  onEngineStatusChange,
  onLanguageChange,
}: {
  onEngineStatusChange: (status: EngineStatus) => void;
  onLanguageChange: (language: Language) => void;
}) {
  const t = useText();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [aboutError, setAboutError] = useState('');
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
        onLanguageChange(loadedSettings.language);
        setEngine(loadedEngine);
        onEngineStatusChange(loadedEngine);
      })
      .catch((cause: unknown) => setError(String(cause)));
  }, [onEngineStatusChange, onLanguageChange]);

  function update<K extends keyof AppSettings>(key: K, value: AppSettings[K]) {
    setSettings((current) => current ? { ...current, [key]: value } : current);
    if (key === 'language') onLanguageChange(value as Language);
    setSaved(false);
  }

  async function showAbout() {
    setAboutOpen(true);
    setAboutError('');
    try {
      setAppVersion(await getVersion());
    } catch (cause) {
      setAboutError(String(cause));
    }
  }

  async function save() {
    if (!settings) return;
    setSaving(true);
    setSaved(false);
    setError('');
    try {
      const savedSettings = await invoke<AppSettings>('save_settings', { settings });
      setSettings(savedSettings);
      const currentEngine = await invoke<EngineStatus>('get_engine_status');
      setEngine(currentEngine);
      onEngineStatusChange(currentEngine);
      setSaved(true);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setSaving(false);
    }
  }

  async function chooseDownloadDirectory() {
    if (!settings) return;
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: t('다운로드 폴더 선택', 'Select download folder'),
        defaultPath: settings.downloadDirectory,
      });
      if (typeof selected === 'string') update('downloadDirectory', selected);
    } catch (cause) {
      setError(String(cause));
    }
  }

  if (!settings && !error) {
    return <EmptyState title={t('설정을 불러오는 중', 'Loading settings')} description={t('저장된 앱 설정을 확인하고 있습니다.', 'Checking saved app settings.')} />;
  }

  return (
    <div className="space-y-5">
      {error && <p role="alert" className="rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-300">{error}</p>}
      {settings && (
        <>
          <Card>
            <div className="flex flex-wrap items-end justify-between gap-4">
              <label className="block min-w-48 text-sm text-zinc-300">
                {t('언어', 'Language')}
                <select
                  value={settings.language}
                  onChange={(event) => update('language', event.target.value as Language)}
                  className="mt-2 h-11 w-full rounded-xl border border-white/10 bg-[#15151c] px-3 text-white"
                >
                  <option value="ko">한국어</option>
                  <option value="en">English</option>
                </select>
              </label>
              <button
                type="button"
                onClick={() => void showAbout()}
                className="h-11 rounded-xl border border-white/10 px-4 text-sm text-zinc-300 hover:bg-white/5"
              >
                About...
              </button>
            </div>
          </Card>
          <Card>
            <h2 className="font-semibold text-white">{t('다운로드 기본값', 'Download defaults')}</h2>
            <div className="mt-5 grid gap-5 sm:grid-cols-2">
              <div className="text-sm text-zinc-300 sm:col-span-2">
                <label htmlFor="download-directory">{t('다운로드 폴더', 'Download folder')}</label>
                <div className="mt-2 flex flex-col gap-2 sm:flex-row">
                  <input
                    id="download-directory"
                    value={settings.downloadDirectory}
                    onChange={(event) => update('downloadDirectory', event.target.value)}
                    className="h-11 min-w-0 flex-1 rounded-xl border border-white/10 bg-black/20 px-3 text-white"
                  />
                  <button
                    type="button"
                    onClick={() => void chooseDownloadDirectory()}
                    className="inline-flex h-11 shrink-0 items-center justify-center gap-2 rounded-xl border border-white/10 px-4 text-sm text-zinc-300 hover:bg-white/5"
                  >
                    <FolderOpen size={16} /> {t('폴더 찾기', 'Browse...')}
                  </button>
                </div>
              </div>
              <label className="text-sm text-zinc-300">
                {t('저장 형식', 'Audio format')}
                <select
                  value={settings.audioFormat}
                  onChange={(event) => update('audioFormat', event.target.value as AppSettings['audioFormat'])}
                  className="mt-2 h-11 w-full rounded-xl border border-white/10 bg-[#15151c] px-3 text-white"
                >
                  <option value="original">{t('원본 오디오', 'Original audio')}</option>
                  <option value="mp3">{t('MP3 변환', 'Convert to MP3')}</option>
                </select>
              </label>
              <label className="text-sm text-zinc-300">
                {t('MP3 음질', 'MP3 quality')}
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
                <span className="mt-1 block text-xs text-zinc-600">{t('원본 스트림보다 높은 음질을 보장하지 않습니다.', 'Quality cannot exceed the source stream.')}</span>
              </label>
              <label className="text-sm text-zinc-300">
                {t('동시 다운로드 수', 'Concurrent downloads')}
                <select
                  value={settings.maxConcurrent}
                  onChange={(event) => update('maxConcurrent', Number(event.target.value))}
                  className="mt-2 h-11 w-full rounded-xl border border-white/10 bg-[#15151c] px-3 text-white"
                >
                  {Array.from({ length: 8 }, (_, index) => index + 1).map((count) => <option key={count} value={count}>{t(`${count}개`, `${count}`)}</option>)}
                </select>
              </label>
              <p className="text-xs leading-5 text-zinc-500 sm:col-span-2">
                {t('MP3 변환과 커버 아트/메타데이터 삽입에는 FFmpeg가 필요합니다. 원본 오디오를 선택하면 변환하지 않습니다.', 'FFmpeg is required for MP3 conversion and embedding artwork or metadata. Original audio is not converted.')}
              </p>
              <div className="space-y-3 sm:col-span-2">
                {([
                  ['overwriteFiles', t('같은 이름의 파일 덮어쓰기', 'Overwrite files with the same name')],
                  ['embedArtwork', t('커버 아트 보존', 'Embed artwork')],
                  ['embedMetadata', t('메타데이터 보존', 'Embed metadata')],
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
                {saving ? t('저장 중…', 'Saving...') : t('설정 저장', 'Save settings')}
              </button>
              {saved && <span role="status" className="text-sm text-emerald-400">{t('설정을 저장했습니다.', 'Settings saved.')}</span>}
            </div>
          </Card>
          <Card>
            <h2 className="font-semibold text-white">{t('다운로드 엔진', 'Download engines')}</h2>
            <p className="mt-2 text-sm text-zinc-500">{t('실행 파일은 저장소에 포함되지 않습니다. yt-dlp는 PATH 검색 또는 아래에 입력한 경로를 사용합니다.', 'Executables are not bundled. yt-dlp is found on PATH or at the configured path below.')}</p>
            <div className="mt-4 grid gap-3 sm:grid-cols-2">
              {tools.map(({ name, status: tool }) => (
                <div key={name} className="rounded-xl border border-white/[.07] bg-black/20 p-4">
                  <p className="text-sm font-medium text-white">{name}</p>
                  <p className={`mt-2 text-sm ${tool?.available ? 'text-emerald-400' : 'text-amber-400'}`}>
                    {tool ? tool.available ? tool.version : tool.error : t('확인 중', 'Checking...')}
                  </p>
                </div>
              ))}
            </div>
            <label className="mt-4 block text-sm text-zinc-300">
              {t('yt-dlp 실행 파일 경로 (선택)', 'yt-dlp executable path (optional)')}
              <input
                value={settings.ytDlpPath}
                onChange={(event) => update('ytDlpPath', event.target.value)}
                placeholder={t('비워 두면 PATH에서 검색', 'Leave empty to search PATH')}
                spellCheck={false}
                className="mt-2 h-11 w-full rounded-xl border border-white/10 bg-black/20 px-3 text-white placeholder:text-zinc-600"
              />
              <span className="mt-1 block text-xs text-zinc-500">
                {t('PATH 검색이 안 되면 yt-dlp 실행 파일의 전체 경로를 입력하고 설정을 저장하세요. Windows 예: C:\\Tools\\yt-dlp.exe', 'If PATH lookup fails, enter the full path to yt-dlp and save. Windows example: C:\\Tools\\yt-dlp.exe')}
              </span>
            </label>
            <div className="mt-4 flex flex-wrap items-center gap-3">
              <button
                type="button"
                onClick={save}
                disabled={saving}
                className="rounded-xl bg-brand px-4 py-2.5 text-sm font-semibold text-white disabled:opacity-50"
              >
                {saving ? t('저장 중…', 'Saving...') : t('엔진 설정 저장', 'Save engine settings')}
              </button>
              {saved && <span role="status" className="text-sm text-emerald-400">{t('설정을 저장했습니다.', 'Settings saved.')}</span>}
            </div>
          </Card>
          {aboutOpen && (
            <div className="fixed inset-0 z-30 grid place-items-center bg-black/70 p-4" role="presentation" onMouseDown={(event) => {
              if (event.target === event.currentTarget) setAboutOpen(false);
            }}>
              <section role="dialog" aria-modal="true" aria-labelledby="about-title" className="w-full max-w-md rounded-2xl border border-white/10 bg-[#15151c] p-6 shadow-2xl">
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <h2 id="about-title" className="text-lg font-semibold text-white">{t('SC-dlp 정보', 'About SC-dlp')}</h2>
                    <p className="mt-2 text-sm text-zinc-300">{t('SoundCloud 다운로드 및 라이브러리 관리 앱', 'SoundCloud downloader and library manager')}</p>
                  </div>
                  <button type="button" onClick={() => setAboutOpen(false)} className="rounded-lg border border-white/10 p-2 text-zinc-400 hover:text-white" aria-label={t('닫기', 'Close')}>
                    <X size={16} />
                  </button>
                </div>
                <p className="mt-4 text-sm text-zinc-400">{t('버전', 'Version')}: {appVersion ?? t('확인 중', 'Loading...')}</p>
                <p className="mt-3 text-xs leading-5 text-zinc-500">© 2026 Unknown Garage Fighter. All rights reserved.</p>
                {aboutError && <p role="alert" className="mt-3 break-all text-xs text-red-300">{aboutError}</p>}
                <div className="mt-6 flex justify-end">
                  <button type="button" onClick={() => setAboutOpen(false)} className="rounded-xl bg-brand px-4 py-2 text-sm font-semibold text-white">{t('닫기', 'Close')}</button>
                </div>
              </section>
            </div>
          )}
        </>
      )}
    </div>
  );
}

export default function App() {
  const [language, setLanguage] = useState<Language>('ko');
  const [languageLoadError, setLanguageLoadError] = useState('');
  const [engineStatus, setEngineStatus] = useState<EngineStatus | null>(null);
  const [engineStatusError, setEngineStatusError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    invoke<AppSettings>('get_settings')
      .then((settings) => {
        if (active) setLanguage(settings.language);
      })
      .catch((cause: unknown) => {
        if (active) setLanguageLoadError(String(cause));
      });
    invoke<EngineStatus>('get_engine_status')
      .then((status) => {
        if (!active) return;
        setEngineStatus(status);
        setEngineStatusError(null);
      })
      .catch((cause: unknown) => {
        if (!active) return;
        setEngineStatus(null);
        setEngineStatusError(String(cause));
      });
    return () => { active = false; };
  }, []);

  const updateEngineStatus = useCallback((status: EngineStatus) => {
    setEngineStatus(status);
    setEngineStatusError(null);
  }, []);

  return (
    <LanguageContext.Provider value={language}>
      <AppShell
        language={language}
        onLanguageChange={setLanguage}
        languageLoadError={languageLoadError}
        engineStatus={engineStatus}
        engineStatusError={engineStatusError}
        onEngineStatusChange={updateEngineStatus}
      />
    </LanguageContext.Provider>
  );
}

function AppShell({
  language,
  onLanguageChange,
  languageLoadError,
  engineStatus,
  engineStatusError,
  onEngineStatusChange,
}: {
  language: Language;
  onLanguageChange: (language: Language) => void;
  languageLoadError: string;
  engineStatus: EngineStatus | null;
  engineStatusError: string | null;
  onEngineStatusChange: (status: EngineStatus) => void;
}) {
  const t = useText();
  const location = useLocation();
  const navigate = useNavigate();
  const page = (pages[location.pathname] ?? pages['/'])[language];
  const Icon = page.icon;

  return (
    <div className="min-h-screen md:flex">
      <aside className="flex shrink-0 flex-col border-b border-white/[.07] bg-[#0e0e14] md:sticky md:top-0 md:h-screen md:w-64 md:border-b-0 md:border-r">
        <div className="flex items-center gap-3 px-5 py-5">
          <div className="grid h-10 w-10 place-items-center rounded-xl bg-brand text-white"><Disc3 size={22} /></div>
          <div><p className="font-semibold tracking-tight text-white">SC-dlp</p><p className="text-xs text-zinc-500">by Unknown Garage Fighter</p></div>
        </div>
        <nav className="flex gap-1 overflow-x-auto px-3 pb-3 md:block md:space-y-1 md:px-3 md:py-5">
          {nav.map(({ to, ko, en, icon: NavIcon }) => (
            <NavLink
              key={to}
              to={to}
              end={to === '/'}
              className={({ isActive }) => `flex shrink-0 items-center gap-3 rounded-xl px-3 py-2.5 text-sm transition ${isActive ? 'bg-white/[.08] text-white' : 'text-zinc-500 hover:bg-white/[.04] hover:text-zinc-200'}`}
            >
              <NavIcon size={17} /><span>{language === 'ko' ? ko : en}</span>
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto hidden p-4 md:block">
          <div className="rounded-xl border border-white/[.07] bg-white/[.025] p-3">
            <div className="flex items-center gap-2 text-xs font-medium text-zinc-400">
              <span className="h-2 w-2 rounded-full bg-brand" /> {t('엔진 정보', 'Engine status')}
            </div>
            <div className="mt-2 space-y-1.5">
              {engineStatus
                ? ([
                    ['yt-dlp', engineStatus.ytDlp],
                    ['FFmpeg', engineStatus.ffmpeg],
                  ] as const).map(([name, tool]) => (
                    <p key={name} className={`truncate text-xs ${tool.available ? 'text-emerald-400' : 'text-amber-400'}`} title={tool.available ? tool.version ?? undefined : tool.error ?? undefined}>
                      {name}: {tool.available ? t('정상 로드', 'Ready') : t('확인 필요', 'Unavailable')}
                    </p>
                  ))
                : <p className="truncate text-xs text-zinc-500" title={engineStatusError ?? undefined}>{engineStatusError ? t('엔진 상태 확인 실패', 'Could not check engines') : t('엔진 상태 확인 중', 'Checking engines...')}</p>}
            </div>
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
            aria-label={t('설정으로 이동', 'Open settings')}
          >
            <Settings size={17} />
          </button>
        </header>
        <main className="mx-auto max-w-[1440px] p-4 sm:p-5 md:p-8">
          {languageLoadError && <p role="alert" className="mb-4 break-all rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-300">{languageLoadError}</p>}
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/playlist" element={<Playlist />} />
            <Route path="/downloads" element={<Downloads />} />
            <Route path="/library" element={<LibraryPage />} />
            <Route path="/settings" element={<SettingsPage onEngineStatusChange={onEngineStatusChange} onLanguageChange={onLanguageChange} />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
          <footer className="mt-8 border-t border-white/[.06] pt-4 text-xs text-zinc-700">
            SC-dlp · by Unknown Garage Fighter
          </footer>
        </main>
      </div>
    </div>
  );
}
