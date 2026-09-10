import type { Metadata } from 'next';
import './globals.css';

const siteUrl = new URL('https://sl-7qx.github.io/s-code-docs/');
const description = 'Product documentation for the local-first S-Code coding-agent execution plane.';

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
    images: [{ url: '/og.png', width: 1200, height: 630, alt: 'S-Code Docs — Build agents that finish the job.' }],
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
