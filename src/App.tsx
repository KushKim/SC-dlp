import {
  Activity,
  ArrowDownToLine,
  ArrowUpRight,
  Disc3,
  Home,
  Library,
  ListMusic,
  Settings,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';
import { Navigate, NavLink, Route, Routes, useLocation, useNavigate } from 'react-router-dom';

type Page = { title: string; subtitle: string; icon: LucideIcon };

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
  return (
    <div className="space-y-6">
      <Card>
        <div className="flex items-center gap-3">
          <div className="rounded-xl bg-brand/10 p-3 text-brand"><Activity size={20} /></div>
          <div>
            <h2 className="font-semibold text-white">다운로드 준비</h2>
            <p className="text-sm text-zinc-500">아직 저장된 설정이나 다운로드 기록이 없습니다.</p>
          </div>
        </div>
        <p className="mt-5 text-sm leading-6 text-zinc-400">
          이 앱은 사용자가 저장 권한을 가진 공개 SoundCloud 콘텐츠를 대상으로 합니다.
          기능이 연결되면 이 화면에 실제 작업과 라이브러리 정보가 표시됩니다.
        </p>
        <NavLink
          to="/playlist"
          className="mt-5 inline-flex items-center gap-2 rounded-xl bg-brand px-4 py-2.5 text-sm font-semibold text-white transition hover:bg-orange-500"
        >
          플레이리스트 보기 <ArrowUpRight size={16} />
        </NavLink>
      </Card>
      <EmptyState
        title="다운로드 기록 없음"
        description="완료된 작업이 없으면 이곳은 비어 있습니다. 샘플 트랙이나 임의의 통계는 표시하지 않습니다."
      />
    </div>
  );
}

function Placeholder({ title }: { title: string }) {
  return (
    <EmptyState
      title={`${title} 기능은 아직 구현되지 않았습니다`}
      description="현재 사용할 수 없는 기능입니다. 백엔드 연결 전까지 동작하는 것처럼 표시하지 않습니다."
    />
  );
}

function Playlist() {
  return (
    <div className="space-y-5">
      <Card>
        <label htmlFor="sc-url" className="mb-3 block text-sm font-medium text-zinc-200">SoundCloud URL</label>
        <div className="flex flex-col gap-3 sm:flex-row">
          <input
            id="sc-url"
            disabled
            placeholder="https://soundcloud.com/..."
            className="h-12 min-w-0 flex-1 rounded-xl border border-white/10 bg-black/20 px-4 text-sm text-white placeholder:text-zinc-600 disabled:cursor-not-allowed"
          />
          <button type="button" disabled className="rounded-xl bg-brand px-5 py-3 text-sm font-semibold text-white opacity-50 disabled:cursor-not-allowed">
            분석 기능 준비 중
          </button>
        </div>
        <p className="mt-3 text-xs text-zinc-600">URL 분석은 아직 구현되지 않았습니다.</p>
      </Card>
      <Placeholder title="플레이리스트 분석" />
    </div>
  );
}

function Downloads() { return <Placeholder title="다운로드 대기열" />; }
function LibraryPage() { return <Placeholder title="음악 라이브러리" />; }
function SettingsPage() { return <Placeholder title="앱 설정" />; }

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
              <span className="h-2 w-2 rounded-full bg-amber-400" /> 준비 중
            </div>
            <p className="mt-2 text-xs leading-5 text-zinc-600">다운로드 엔진이 아직 연결되지 않았습니다.</p>
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
            SoundCloud Downloader · 초기 안정화 단계
          </footer>
        </main>
      </div>
    </div>
  );
}
