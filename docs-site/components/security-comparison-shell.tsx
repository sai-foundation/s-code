import {
  ArrowLeft,
  ArrowRight,
  Check,
  EyeOff,
  FileCheck2,
  Fingerprint,
  KeyRound,
  LockKeyhole,
  Network,
  Server,
  ShieldCheck,
  TerminalSquare,
  TriangleAlert,
  WifiOff,
} from 'lucide-react';
import Link from 'next/link';
import { SiteHeader } from '@/components/site-header';
import type { DocEntry } from '@/lib/generated-docs';

const threatCards = [
  {
    icon: WifiOff,
    number: '01',
    title: '提示注入外传密钥',
    copy: '文件、网络与批准是三道独立边界。',
  },
  {
    icon: TerminalSquare,
    number: '02',
    title: '恶意依赖越界写入',
    copy: '内置命令工具及其普通子进程继承 OS 沙箱。',
  },
  {
    icon: FileCheck2,
    number: '03',
    title: '并发修改静默覆盖',
    copy: '陈旧摘要让写入在落盘前失败。',
  },
  {
    icon: EyeOff,
    number: '04',
    title: '浏览器脚本窃取密钥',
    copy: '长期凭据不进入浏览器 JavaScript。',
  },
];

const principles = [
  { icon: LockKeyhole, label: 'OS 级命令隔离' },
  { icon: WifiOff, label: '命令默认断网' },
  { icon: Fingerprint, label: '带版本的文件编辑' },
  { icon: ShieldCheck, label: '批准与审计一体' },
];

export function SecurityComparisonShell({
  doc,
  children,
}: {
  doc: DocEntry;
  children: React.ReactNode;
}) {
  return (
    <div className="min-h-screen bg-background text-foreground" lang="zh-CN">
      <SiteHeader activeSlug={doc.slug} />

      <main>
        <section className="security-hero border-b border-border px-5 py-12 sm:px-10 sm:py-16 lg:py-20">
          <div className="mx-auto max-w-[1160px]">
            <Link href="/" className="security-back-link">
              <ArrowLeft className="size-3.5" /> 返回 S-Code 文档
            </Link>

            <div className="mt-10 grid gap-10 lg:grid-cols-[minmax(0,1.1fr)_minmax(360px,.9fr)] lg:items-center">
              <div>
                <div className="security-kicker">
                  <ShieldCheck className="size-4" /> 隐私与安全设计说明
                </div>
                <h1 className="mt-5 max-w-4xl text-balance text-[clamp(2.65rem,7vw,5.7rem)] font-semibold leading-[.94] tracking-[-.065em]">
                  安全不是一句
                  <br />
                  “本地运行”。
                </h1>
                <p className="mt-7 max-w-2xl text-pretty text-lg leading-8 text-muted-foreground sm:text-xl">
                  {doc.description}
                </p>
                <div className="mt-8 flex flex-wrap gap-2">
                  {principles.map(({ icon: Icon, label }) => (
                    <span key={label} className="security-pill">
                      <Icon className="size-3.5" />
                      {label}
                    </span>
                  ))}
                </div>
              </div>

              <div
                className="security-boundary-map"
                aria-label="S-Code credential and execution boundary diagram"
              >
                <div className="security-boundary-head">
                  <span>边界不是口号</span>
                  <span className="flex items-center gap-1 text-emerald-300">
                    <Check className="size-3.5" /> 可由源码核验
                  </span>
                </div>
                <div className="security-boundary-stage">
                  <div className="security-node security-node-key">
                    <KeyRound className="size-5" />
                    <div>
                      <small>凭据边界</small>
                      <strong>Direct 或独立 Proxy</strong>
                      <span>Direct: daemon 可访问 · Proxy: 仅 proxy 持有</span>
                    </div>
                  </div>
                  <div className="security-flow-line">
                    <span>Provider 请求</span>
                    <ArrowRight className="size-4" />
                  </div>
                  <div className="security-node">
                    <Server className="size-5" />
                    <div>
                      <small>执行边界</small>
                      <strong>S-Code execution service</strong>
                      <span>工具 · 策略 · 批准 · 审计</span>
                    </div>
                  </div>
                  <div className="security-flow-split">
                    <div>
                      <span>HttpOnly Cookie</span>
                      <ArrowRight className="size-4" />
                    </div>
                    <div>
                      <span>本地连接</span>
                      <ArrowRight className="size-4" />
                    </div>
                  </div>
                  <div className="grid grid-cols-2 gap-2">
                    <div className="security-client">
                      <EyeOff className="size-4" />
                      <strong>Local Web</strong>
                      <span>看不到 Provider Key</span>
                    </div>
                    <div className="security-client">
                      <TerminalSquare className="size-4" />
                      <strong>CLI</strong>
                      <span>共享同一证据链</span>
                    </div>
                  </div>
                </div>
                <div className="security-boundary-foot">
                  <Network className="size-4" />{' '}
                  需要网络时扩大的是显式能力，而不是模型的愿望。
                </div>
              </div>
            </div>
          </div>
        </section>

        <section className="border-b border-border bg-surface/55 px-5 py-6 sm:px-10">
          <div className="mx-auto grid max-w-[1160px] gap-px overflow-hidden border border-border bg-border sm:grid-cols-2 xl:grid-cols-4">
            {threatCards.map(({ icon: Icon, number, title, copy }) => (
              <article key={number} className="bg-background p-5 sm:p-6">
                <div className="flex items-center justify-between">
                  <Icon className="size-5 text-primary" />
                  <span className="font-mono text-[11px] text-muted-foreground">
                    {number}
                  </span>
                </div>
                <h2 className="mt-8 text-base font-semibold tracking-[-.025em]">
                  {title}
                </h2>
                <p className="mt-2 text-sm leading-6 text-muted-foreground">
                  {copy}
                </p>
              </article>
            ))}
          </div>
        </section>

        <section className="px-5 py-12 sm:px-10 sm:py-16">
          <div className="mx-auto grid max-w-[1160px] gap-12 xl:grid-cols-[minmax(0,820px)_220px] xl:items-start">
            <article className="security-content doc-content min-w-0">
              {children}
            </article>

            <aside className="security-toc xl:sticky xl:top-24">
              <p className="mb-4 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[.12em] text-muted-foreground">
                <ShieldCheck className="size-3.5 text-primary" /> 本页内容
              </p>
              <nav aria-label="本页目录">
                <ul className="space-y-2.5 text-sm text-muted-foreground">
                  {doc.headings.map((heading) => (
                    <li key={heading.id}>
                      <a href={`#${heading.id}`}>{heading.label}</a>
                    </li>
                  ))}
                </ul>
              </nav>
              <div className="mt-8 border-t border-border pt-5 text-xs leading-5 text-muted-foreground">
                <TriangleAlert className="mb-2 size-4 text-amber-600" />
                只比较官方公开文档与可定位源码，不把未知项写成竞品缺失。
              </div>
            </aside>
          </div>
        </section>

        <section className="border-t border-border bg-foreground px-5 py-10 text-background sm:px-10">
          <div className="mx-auto flex max-w-[1160px] flex-col gap-5 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="text-xs font-semibold uppercase tracking-[.14em] text-primary">
                Trust through boundaries
              </p>
              <p className="mt-2 text-xl font-semibold tracking-[-.03em]">
                看得见边界，才能真正信任自动化。
              </p>
            </div>
            <Link
              href="/docs/security"
              className="inline-flex h-10 items-center justify-center gap-2 border border-background/30 px-4 text-sm font-semibold transition hover:bg-background/10"
            >
              阅读完整安全模型 <ArrowRight className="size-4" />
            </Link>
          </div>
        </section>
      </main>
    </div>
  );
}
