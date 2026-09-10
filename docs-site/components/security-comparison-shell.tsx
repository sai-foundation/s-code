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
    title: 'Prompt injection targets secrets',
    copy: 'File access, networking and approval are separate checks.',
  },
  {
    icon: TerminalSquare,
    number: '02',
    title: 'Dependencies write outside the workspace',
    copy: 'Built-in commands and ordinary child processes share the OS sandbox.',
  },
  {
    icon: FileCheck2,
    number: '03',
    title: 'Concurrent edits overwrite changes',
    copy: 'A stale content digest rejects the edit before writing.',
  },
  {
    icon: EyeOff,
    number: '04',
    title: 'Browser scripts target provider keys',
    copy: 'Long-lived credentials stay out of browser JavaScript.',
  },
];

const principles = [
  { icon: LockKeyhole, label: 'OS command isolation' },
  { icon: WifiOff, label: 'Network off by default' },
  { icon: Fingerprint, label: 'Version-checked edits' },
  { icon: ShieldCheck, label: 'Approvals with audit records' },
];

export function SecurityComparisonShell({
  doc,
  children,
}: {
  doc: DocEntry;
  children: React.ReactNode;
}) {
  return (
    <div className="min-h-screen bg-background text-foreground" lang="en">
      <SiteHeader activeSlug={doc.slug} />

      <main>
        <section className="security-hero border-b border-border px-5 py-12 sm:px-10 sm:py-16 lg:py-20">
          <div className="mx-auto max-w-[1160px]">
            <Link href="/" className="security-back-link">
              <ArrowLeft className="size-3.5" /> Back to S-Code docs
            </Link>

            <div className="mt-10 grid gap-10 lg:grid-cols-[minmax(0,1.1fr)_minmax(360px,.9fr)] lg:items-center">
              <div>
                <div className="security-kicker">
                  <ShieldCheck className="size-4" /> Privacy and security
                </div>
                <h1 className="mt-5 max-w-4xl text-balance text-[clamp(2.25rem,5vw,3.8rem)] font-semibold leading-[1.05] tracking-[-.065em]">
                  Protect credentials.
                  <br />
                  Constrain execution.
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
                  <span>Execution boundaries</span>
                  <span className="flex items-center gap-1 text-emerald-300">
                    <Check className="size-3.5" /> Source available
                  </span>
                </div>
                <div className="security-boundary-stage">
                  <div className="security-node security-node-key">
                    <KeyRound className="size-5" />
                    <div>
                      <small>Model credentials</small>
                      <strong>Direct or separate proxy</strong>
                      <span>Held by the daemon in direct mode, or by the proxy</span>
                    </div>
                  </div>
                  <div className="security-flow-line">
                    <span>Model requests</span>
                    <ArrowRight className="size-4" />
                  </div>
                  <div className="security-node">
                    <Server className="size-5" />
                    <div>
                      <small>Execution boundary</small>
                      <strong>S-Code execution service</strong>
                      <span>Tools · Policy · Approvals · Audit</span>
                    </div>
                  </div>
                  <div className="security-flow-split">
                    <div>
                      <span>HttpOnly Cookie</span>
                      <ArrowRight className="size-4" />
                    </div>
                    <div>
                      <span>Local connection</span>
                      <ArrowRight className="size-4" />
                    </div>
                  </div>
                  <div className="grid grid-cols-2 gap-2">
                    <div className="security-client">
                      <EyeOff className="size-4" />
                      <strong>Local Web</strong>
                      <span>No provider key</span>
                    </div>
                    <div className="security-client">
                      <TerminalSquare className="size-4" />
                      <strong>CLI</strong>
                      <span>Shared session evidence</span>
                    </div>
                  </div>
                </div>
                <div className="security-boundary-foot">
                  <Network className="size-4" />{' '}
                  Network access requires an explicit capability grant.
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
                <ShieldCheck className="size-3.5 text-primary" /> On this page
              </p>
              <nav aria-label="Table of contents">
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
                Comparisons use published documentation and source. Undocumented does not mean unavailable.
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
                Inspect the boundaries behind each action.
              </p>
            </div>
            <Link
              href="/docs/security"
              className="inline-flex h-10 items-center justify-center gap-2 border border-background/30 px-4 text-sm font-semibold transition hover:bg-background/10"
            >
              Read the security model <ArrowRight className="size-4" />
            </Link>
          </div>
        </section>
      </main>
    </div>
  );
}
