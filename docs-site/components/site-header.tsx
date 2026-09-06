'use client';

import { useEffect, useState } from 'react';
import { BookOpen, GitFork, Menu, Moon, Search, ShieldCheck, Sun } from 'lucide-react';
import Link from 'next/link';
import { Button } from '@/components/ui/button';
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from '@/components/ui/sheet';
import { docGroups, docs } from '@/lib/generated-docs';

function Brand() {
  return (
    <Link href="/" className="flex shrink-0 items-center gap-3" aria-label="S-Code documentation home">
      <span className="brand-mark" aria-hidden="true"><span /></span>
      <span className="text-[15px] font-semibold tracking-[-0.02em]">S-Code</span>
      <span className="hidden border-l border-border pl-3 text-sm text-muted-foreground sm:inline">Documentation</span>
    </Link>
  );
}

function MobileNavigation({ activeSlug }: { activeSlug?: string }) {
  return (
    <Sheet>
      <SheetTrigger render={<Button variant="ghost" size="icon" className="lg:hidden" aria-label="Open navigation" />}>
        <Menu />
      </SheetTrigger>
      <SheetContent side="left" className="w-[88vw] max-w-[340px] bg-background">
        <SheetHeader className="border-b border-border px-5 py-5 text-left">
          <SheetTitle><Brand /></SheetTitle>
          <SheetDescription>S-Code documentation</SheetDescription>
        </SheetHeader>
        <nav className="overflow-y-auto px-5 py-5" aria-label="Mobile documentation navigation">
          {docGroups.map((group) => (
            <section key={group} className="mb-7">
              <h2 className="mb-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-muted-foreground">{group}</h2>
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
      </SheetContent>
    </Sheet>
  );
}

export function SiteHeader({ activeSlug }: { activeSlug?: string }) {
  const [searchOpen, setSearchOpen] = useState(false);
  const [dark, setDark] = useState(false);

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      setDark(document.documentElement.classList.contains('dark'));
    });
    const handleKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setSearchOpen((open) => !open);
      }
    };
    window.addEventListener('keydown', handleKey);
    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener('keydown', handleKey);
    };
  }, []);

  const toggleTheme = () => {
    const next = !dark;
    document.documentElement.classList.toggle('dark', next);
    window.localStorage.setItem('s-code-docs-theme', next ? 'dark' : 'light');
    setDark(next);
  };

  return (
    <>
      <header className="sticky top-0 z-40 border-b border-border/80 bg-background/92 backdrop-blur-xl">
        <div className="mx-auto flex h-16 max-w-[1600px] items-center gap-3 px-4 sm:px-5 lg:px-8">
          <MobileNavigation activeSlug={activeSlug} />
          <Brand />
          <button
            type="button"
            className="mx-auto hidden h-9 w-full max-w-[460px] items-center gap-2 rounded-md border border-border bg-surface px-3 text-left text-sm text-muted-foreground transition hover:border-primary/40 hover:text-foreground md:flex"
            onClick={() => setSearchOpen(true)}
          >
            <Search className="size-4" />
            <span>Search documentation</span>
            <kbd className="ml-auto rounded border border-border bg-background px-1.5 py-0.5 font-mono text-[10px]">⌘K</kbd>
          </button>
          <div className="ml-auto flex items-center gap-1 md:ml-0">
            <Link href="/docs/privacy-security-comparison" className="header-link hidden px-2 xl:inline-flex">
              <ShieldCheck className="size-4" /> 隐私与安全
            </Link>
            <Button variant="ghost" size="icon" className="md:hidden" aria-label="Search documentation" onClick={() => setSearchOpen(true)}><Search /></Button>
            <Button variant="ghost" size="icon" aria-label={dark ? 'Use light theme' : 'Use dark theme'} onClick={toggleTheme}>
              {dark ? <Sun /> : <Moon />}
            </Button>
            <a href="https://github.com/sl-7qx/s-code" className="header-link inline-flex p-2" aria-label="Open the S-Code repository on GitHub">
              <GitFork className="size-4" />
              <span className="hidden xl:inline">GitHub</span>
            </a>
          </div>
        </div>
      </header>

      <CommandDialog open={searchOpen} onOpenChange={setSearchOpen} title="Search S-Code documentation" description="Search product guides and concepts">
        <CommandInput placeholder="Search guides, concepts, and operations…" />
        <CommandList>
          <CommandEmpty>No documentation matched your search.</CommandEmpty>
          {docGroups.map((group) => (
            <CommandGroup key={group} heading={group}>
              {docs.filter((doc) => doc.group === group).map((doc) => (
                <CommandItem
                  key={doc.slug}
                  value={`${doc.title} ${doc.description} ${doc.keywords.join(' ')}`}
                  onSelect={() => { window.location.assign(`/docs/${doc.slug}`); }}
                  className="items-start py-2.5"
                >
                  <BookOpen className="mt-0.5 size-4 text-primary" />
                  <span><span className="block font-medium">{doc.title}</span><span className="mt-0.5 block text-xs text-muted-foreground">{doc.description}</span></span>
                </CommandItem>
              ))}
            </CommandGroup>
          ))}
        </CommandList>
      </CommandDialog>
    </>
  );
}
