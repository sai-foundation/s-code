import {
  ArrowRight,
  BookOpen,
  Braces,
  Check,
  ShieldCheck,
  TerminalSquare,
  Workflow,
} from 'lucide-react';
import Link from 'next/link';
import { DocsSidebar } from '@/components/docs-sidebar';
import { SiteHeader } from '@/components/site-header';

const capabilities = [
  {
    icon: Workflow,
    eyebrow: 'One execution plane',
    title: 'Every client sees the same work',
    copy: 'CLI and Local Web share sessions, tools, approvals, events, and durable history.',
  },
  {
    icon: ShieldCheck,
    eyebrow: 'Policy in the loop',
    title: 'Boundaries are part of execution',
    copy: 'Guarded edits, sandboxed commands, approvals, and audit evidence travel together.',
    href: '/docs/privacy-security-comparison',
  },
  {
    icon: Braces,
    eyebrow: 'Model portable',
    title: 'Keep the harness, change the model',
    copy: 'Connect an OpenAI-compatible endpoint without replacing the agent runtime.',
  },
];

export default function Home() {
  return (
    <div className="min-h-screen bg-background text-foreground">
      <SiteHeader activeSlug="introduction" />

      <div className="mx-auto grid max-w-[1600px] grid-cols-1 lg:grid-cols-[268px_minmax(0,1fr)]">
        <DocsSidebar activeSlug="introduction" />

        <main className="min-w-0">
          <section className="hero-grid border-b border-border px-5 py-14 sm:px-10 sm:py-20 xl:px-16">
            <div className="mx-auto max-w-[1040px]">
              <div className="mb-7 inline-flex items-center gap-2 border border-primary/20 bg-primary/6 px-2.5 py-1 text-xs font-medium text-primary">
                <span className="size-1.5 rounded-full bg-primary" />
                S-Code · Developer Preview
              </div>
              <h1 className="max-w-4xl text-balance text-[clamp(2.8rem,7vw,6.5rem)] font-semibold leading-[0.93] tracking-[-0.065em]">
                Safe, Speedy,<br />Self-evolving coding agent.
              </h1>
              <p className="mt-7 max-w-2xl text-pretty text-lg leading-8 text-muted-foreground sm:text-xl">
                Create, edit, and test code in your terminal or browser, with guarded file changes, reusable project experience, and efficient tool execution.
              </p>
              <div className="mt-9 flex flex-wrap gap-3">
                <Link className="primary-action" href="/docs/quick-start">Get started <ArrowRight className="size-4" /></Link>
                <Link className="secondary-action" href="/docs/core-concepts"><BookOpen className="size-4" /> Explore concepts</Link>
              </div>
            </div>
          </section>

          <section className="px-5 py-12 sm:px-10 xl:px-16">
            <div className="mx-auto max-w-[1040px]">
              <div className="grid gap-px overflow-hidden border border-border bg-border md:grid-cols-3">
                {capabilities.map((capability) => {
                  const Icon = capability.icon;
                  return (
                    <article key={capability.title} className="bg-background p-7 lg:p-8">
                      <Icon className="mb-8 size-5 text-primary" strokeWidth={1.8} />
                      <p className="mb-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-primary">{capability.eyebrow}</p>
                      <h2 className="text-lg font-semibold tracking-[-0.025em]">{capability.title}</h2>
                      <p className="mt-3 text-sm leading-6 text-muted-foreground">{capability.copy}</p>
                      {'href' in capability && capability.href && <Link href={capability.href} className="mt-5 inline-flex items-center gap-1.5 text-xs font-semibold text-primary">Security comparison <ArrowRight className="size-3.5" /></Link>}
                    </article>
                  );
                })}
              </div>

              <section id="quick-start" className="mt-16 grid gap-8 lg:grid-cols-[minmax(0,1fr)_360px] lg:items-start">
                <div>
                  <p className="section-label">Quick start</p>
                  <h2 className="mt-3 text-3xl font-semibold tracking-[-0.045em] sm:text-4xl">From source to first session</h2>
                  <p className="mt-4 max-w-xl text-base leading-7 text-muted-foreground">Install S-Code, then choose the CLI or Local Web. Both clients connect to the same local execution service.</p>
                  <ol className="mt-8 space-y-4">
                    {['Clone the S-Code repository', 'Install the local application', 'Start the client you prefer'].map((item, index) => (
                      <li key={item} className="flex items-center gap-3 text-sm"><span className="flex size-6 items-center justify-center border border-border bg-surface font-mono text-[11px] text-muted-foreground">{index + 1}</span>{item}</li>
                    ))}
                  </ol>
                </div>
                <div className="code-window">
                  <div className="flex items-center justify-between border-b border-white/10 px-4 py-3 text-xs text-slate-400">
                    <span className="flex items-center gap-2"><TerminalSquare className="size-3.5" /> Terminal</span>
                    <span className="flex items-center gap-1 text-emerald-300"><Check className="size-3" /> macOS + Linux</span>
                  </div>
                  <pre className="overflow-x-auto p-5 font-mono text-[13px] leading-7 text-slate-200"><code><span className="text-slate-500">$</span> git clone https://github.com/sai-foundation/s-code.git{`\n`}<span className="text-slate-500">$</span> cd s-code{`\n`}<span className="text-slate-500">$</span> scripts/install-from-source.sh{`\n`}<span className="text-slate-500">$</span> export PATH=&quot;$HOME/.local/bin:$PATH&quot;{`\n`}<span className="text-slate-500">$</span> s-code setup{`\n`}<span className="text-slate-500">$</span> s-code doctor{`\n`}<span className="text-slate-500">$</span> s-code</code></pre>
                </div>
              </section>

              <section id="architecture" className="mt-20 border-t border-border pt-12">
                <p className="section-label">Architecture at a glance</p>
                <div className="mt-5 flex flex-wrap items-center gap-2 text-sm">
                  {['Model endpoint', 'Execution service', 'CLI · Local Web · IDE'].map((item, index) => (
                    <div key={item} className="contents">
                      <span className="border border-border bg-surface px-4 py-3 font-medium">{item}</span>
                      {index < 2 && <ArrowRight className="size-4 text-muted-foreground" aria-hidden="true" />}
                    </div>
                  ))}
                </div>
              </section>
            </div>
          </section>
        </main>
      </div>
    </div>
  );
}
