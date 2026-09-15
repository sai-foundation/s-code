import type { Metadata } from 'next';
import './globals.css';

const siteUrl = new URL('https://sai-foundation.github.io/s-code-docs/');
const description = 'S-Code: Safe, self-evolving, and swift coding agent. Guides for your terminal and browser.';

export const metadata: Metadata = {
  metadataBase: siteUrl,
  title: {
    default: 'S-Code Documentation',
    template: '%s · S-Code Docs',
  },
  description,
  alternates: { canonical: '/' },
  openGraph: {
    type: 'website',
    url: '/',
    title: 'S-Code Documentation',
    description,
    siteName: 'S-Code Docs',
    images: [{ url: '/og.png', width: 1731, height: 909, alt: 'S-Code Docs — Safe, self-evolving, and swift coding agent.' }],
  },
  twitter: {
    card: 'summary_large_image',
    title: 'S-Code Documentation',
    description,
    images: ['/og.png'],
  },
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: `try{const t=localStorage.getItem('s-code-docs-theme');if(t==='dark'||(!t&&matchMedia('(prefers-color-scheme: dark)').matches))document.documentElement.classList.add('dark')}catch{}` }} />
      </head>
      <body className="antialiased">{children}</body>
    </html>
  );
}
