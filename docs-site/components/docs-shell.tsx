import { ArrowLeft, ArrowRight } from 'lucide-react';
import Link from 'next/link';
import { DocsSidebar } from '@/components/docs-sidebar';
import { SiteHeader } from '@/components/site-header';
import { getAdjacentDocs, type DocEntry } from '@/lib/generated-docs';

export function DocsShell({ doc, children }: { doc: DocEntry; children: React.ReactNode }) {
  const { previous, next } = getAdjacentDocs(doc.slug);
  return (
    <div className="min-h-screen bg-background text-foreground">
      <SiteHeader activeSlug={doc.slug} />
      <div className="mx-auto grid max-w-[1600px] grid-cols-1 lg:grid-cols-[268px_minmax(0,1fr)] xl:grid-cols-[268px_minmax(0,1fr)_220px]">
        <DocsSidebar activeSlug={doc.slug} />
        <main className="min-w-0 px-5 py-10 sm:px-10 sm:py-14 xl:px-14">
          <article className="mx-auto max-w-[790px]">
            <nav aria-label="Breadcrumb" className="mb-7 flex items-center gap-2 text-xs text-muted-foreground">
              <Link href="/" className="hover:text-foreground">Docs</Link><span aria-hidden="true">/</span><span>{doc.group}</span>
            </nav>
            <p className="section-label">{doc.group}</p>
            <h1 className="mt-3 text-balance text-4xl font-semibold tracking-[-0.05em] sm:text-5xl">{doc.title}</h1>
            <p className="mt-5 max-w-2xl text-lg leading-8 text-muted-foreground">{doc.description}</p>
            <div className="doc-content mt-12">{children}</div>
            <nav aria-label="Previous and next documentation" className="mt-16 grid gap-3 border-t border-border pt-8 sm:grid-cols-2">
              {previous ? (
                <Link href={`/docs/${previous.slug}`} className="doc-next group">
                  <span className="doc-next-label"><ArrowLeft className="size-3.5" /> Previous</span>
                  <strong>{previous.title}</strong>
                </Link>
              ) : <span />}
              {next && (
                <Link href={`/docs/${next.slug}`} className="doc-next group sm:text-right">
                  <span className="doc-next-label sm:justify-end">Next <ArrowRight className="size-3.5" /></span>
                  <strong>{next.title}</strong>
                </Link>
              )}
            </nav>
          </article>
        </main>
        <aside className="sticky top-16 hidden h-[calc(100vh-4rem)] border-l border-border px-6 py-10 xl:block">
          <p className="mb-4 text-[11px] font-semibold uppercase tracking-[0.12em] text-muted-foreground">On this page</p>
          <nav aria-label="On this page">
            <ul className="space-y-2.5 text-sm text-muted-foreground">
              {doc.headings.map((heading) => <li key={heading.id}><a className="transition hover:text-foreground" href={`#${heading.id}`}>{heading.label}</a></li>)}
            </ul>
          </nav>
          <a href={`https://github.com/sai-foundation/s-code/edit/main/${doc.sourcePath}`} className="mt-9 block border-t border-border pt-5 text-xs text-muted-foreground hover:text-foreground">Improve this page →</a>
        </aside>
      </div>
    </div>
  );
}
