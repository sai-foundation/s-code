import { docGroups, docs } from '@/lib/generated-docs';
import Link from 'next/link';

export function DocsSidebar({ activeSlug }: { activeSlug?: string }) {
  return (
    <aside className="sticky top-16 hidden h-[calc(100vh-4rem)] border-r border-border px-6 py-8 lg:block">
      <nav aria-label="Documentation navigation" className="space-y-8">
        {docGroups.map((group) => (
          <section key={group}>
            <h2 className="mb-3 text-[11px] font-semibold uppercase tracking-[0.12em] text-muted-foreground">{group}</h2>
            <ul className="space-y-1">
              {docs.filter((doc) => doc.group === group).map((doc) => (
                <li key={doc.slug}>
                  <Link className={`nav-link ${doc.slug === activeSlug ? 'nav-link-active' : ''}`} href={`/docs/${doc.slug}`}>{doc.shortTitle}</Link>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </nav>
      <div className="absolute bottom-7 left-6 right-6 border-t border-border pt-5 text-xs leading-5 text-muted-foreground">
        <p className="font-medium text-foreground">Apache-2.0 Community</p>
        <p>Private release-candidate development</p>
      </div>
    </aside>
  );
}
