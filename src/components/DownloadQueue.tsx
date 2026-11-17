import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useEffect, useState } from 'react';
import { AlertCircle, Ban, CheckCircle2, Clock3, Download, LoaderCircle } from 'lucide-react';

type JobStatus = 'queued' | 'downloading' | 'completed' | 'failed' | 'cancelled';
type DownloadJob = {
  id: string;
  url: string;
  title: string;
  status: JobStatus;
  progress: number;
  speed: string | null;
  eta: string | null;
  output_path: string | null;
  error: string | null;
};
type ProgressEvent = { job: DownloadJob };

const statusLabels: Record<JobStatus, string> = {
  queued: '대기 중', downloading: '다운로드 중', completed: '완료', failed: '실패', cancelled: '취소됨'
};

export function QueueTrackButton({ url }: { url: string | null }) {
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);
  async function queueTrack() {
    if (!url) return;
    setBusy(true);
    setMessage('');
    try {
      await invoke<DownloadJob>('queue_download', { url });
      setMessage('대기열에 추가했습니다.');
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  }
  return <div className="flex items-center gap-2"><button type="button" onClick={queueTrack} disabled={!url || busy} title={!url ? '이 트랙의 다운로드 URL이 없습니다.' : '다운로드 대기열에 추가'} className="inline-flex items-center gap-1.5 rounded-lg border border-white/10 bg-white/[.04] px-3 py-2 text-xs font-medium text-zinc-200 transition hover:bg-white/[.08] disabled:cursor-not-allowed disabled:opacity-40"><Download size={14}/>{busy ? '추가 중…' : '다운로드'}</button>{message && <span role="status" className="max-w-40 truncate text-xs text-zinc-500">{message}</span>}</div>;
}

export default function DownloadQueue() {
  const [jobs, setJobs] = useState<DownloadJob[]>([]);
  const [error, setError] = useState('');
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let active = true;
    async function setup() {
      try {
        const stop = await listen<ProgressEvent>('download-progress', ({ payload }) => {
          setJobs((current) => {
            const index = current.findIndex((job) => job.id === payload.job.id);
            if (index < 0) return [payload.job, ...current];
            const next = [...current];
            next[index] = payload.job;
            return next;
          });
        });
        if (!active) { stop(); return; }
        unlisten = stop;
        const current = await invoke<DownloadJob[]>('list_downloads');
        if (active) setJobs(current.reverse());
      } catch (reason) {
        if (active) setError(`대기열을 불러오지 못했습니다: ${String(reason)}`);
      }
    }
    void setup();
    return () => { active = false; unlisten?.(); };
  }, []);

  async function cancel(id: string) {
    try {
      await invoke('cancel_download', { id });
      setJobs((current) => current.map((job) => job.id === id ? { ...job, status: 'cancelled' } : job));
    } catch (reason) {
      setError(`작업 취소 실패: ${String(reason)}`);
    }
  }

  return <div className="space-y-5"><div className="flex items-end justify-between"><div><p className="text-sm text-zinc-500">실행 중인 작업과 완료된 기록</p><h2 className="mt-1 text-xl font-semibold text-white">다운로드 대기열</h2></div><span className="rounded-full border border-white/10 px-3 py-1 text-xs text-zinc-400">{jobs.filter((job) => ['queued', 'downloading'].includes(job.status)).length} 활성 작업</span></div>{error && <div role="alert" className="rounded-xl border border-rose-500/20 bg-rose-500/[.06] p-3 text-sm text-rose-300">{error}</div>}{jobs.length === 0 ? <div className="flex min-h-64 flex-col items-center justify-center rounded-2xl border border-dashed border-white/10 text-center"><div className="mb-3 rounded-xl bg-white/[.04] p-3 text-zinc-500"><Download size={22}/></div><p className="font-medium text-zinc-300">대기열이 비어 있습니다</p><p className="mt-1 text-sm text-zinc-600">플레이리스트 화면에서 트랙을 대기열에 추가하세요.</p></div> : <div className="space-y-3">{jobs.map((job) => <article key={job.id} className="rounded-2xl border border-white/[.07] bg-surface p-4"><div className="flex flex-wrap items-start justify-between gap-3"><div className="flex min-w-0 items-center gap-3"><div className="rounded-xl bg-brand/10 p-3 text-brand">{job.status === 'downloading' ? <LoaderCircle className="animate-spin" size={19}/> : job.status === 'completed' ? <CheckCircle2 size={19}/> : job.status === 'failed' ? <AlertCircle size={19}/> : <Clock3 size={19}/>}</div><div className="min-w-0"><p className="truncate text-sm font-medium text-white">{job.title}</p><p className="mt-1 truncate text-xs text-zinc-600">{job.url}</p></div></div><div className="flex items-center gap-2"><span className="rounded-full border border-white/10 bg-white/[.04] px-2.5 py-1 text-xs text-zinc-300">{statusLabels[job.status]}</span>{['queued','downloading'].includes(job.status) && <button type="button" onClick={() => void cancel(job.id)} className="rounded-lg border border-white/10 p-2 text-zinc-400 transition hover:border-rose-500/30 hover:text-rose-300" aria-label="다운로드 취소"><Ban size={15}/></button>}</div></div><div className="mt-4"><div className="mb-2 flex justify-between text-xs text-zinc-500"><span>{job.speed ?? (job.status === 'queued' ? '대기 중' : '진행률')}</span><span>{job.progress.toFixed(1)}%{job.eta ? ` · 남은 시간 ${job.eta}` : ''}</span></div><div className="h-2 overflow-hidden rounded-full bg-white/[.06]"><div className={`h-full rounded-full transition-all ${job.status === 'failed' ? 'bg-rose-500' : 'bg-gradient-to-r from-brand to-orange-400'}`} style={{ width: `${Math.min(100, Math.max(0, job.progress))}%` }}/></div>{job.error && <p className="mt-3 whitespace-pre-wrap text-xs text-rose-300">{job.error}</p>}{job.output_path && <p className="mt-2 truncate text-xs text-zinc-600">저장 위치: {job.output_path}</p>}</div></article>)}</div>}</div>;
}
