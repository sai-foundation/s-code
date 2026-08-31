import type { Metadata } from 'next';
import './globals.css';

const siteUrl = new URL('https://opencoding-community-docs.shilong86.chatgpt.site');
const description = 'Product documentation for the local-first Opencoding Community coding-agent execution plane.';

export const metadata: Metadata = {
  metadataBase: siteUrl,
  title: {
    default: 'Opencoding Community Documentation',
    template: '%s · Opencoding Community Docs',
  },
  description,
  alternates: { canonical: '/' },
  openGraph: {
    type: 'website',
    url: '/',
    title: 'Opencoding Community Documentation',
    description,
    siteName: 'Opencoding Community Docs',
    images: [{ url: '/og.png', width: 1200, height: 630, alt: 'Opencoding Community Docs — Build agents that finish the job.' }],
  },
  twitter: {
    card: 'summary_large_image',
    title: 'Opencoding Community Documentation',
    description,
    images: ['/og.png'],
  },
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: `try{const t=localStorage.getItem('opencoding-docs-theme');if(t==='dark'||(!t&&matchMedia('(prefers-color-scheme: dark)').matches))document.documentElement.classList.add('dark')}catch{}` }} />
      </head>
      <body className="antialiased">{children}</body>
    </html>
  );
}
