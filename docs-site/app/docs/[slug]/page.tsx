import type { Metadata } from 'next';
import { notFound } from 'next/navigation';
import { DocsShell } from '@/components/docs-shell';
import { SecurityComparisonShell } from '@/components/security-comparison-shell';
import { docs, getDoc } from '@/lib/generated-docs';

export function generateStaticParams() {
  return docs.map((doc) => ({ slug: doc.slug }));
}

export async function generateMetadata({
  params,
}: {
  params: Promise<{ slug: string }>;
}): Promise<Metadata> {
  const { slug } = await params;
  const doc = getDoc(slug);
  if (!doc) return {};
  if (doc.slug === 'privacy-security-comparison') {
    return {
      title: doc.title,
      description: doc.description,
      alternates: { canonical: `/docs/${doc.slug}` },
      openGraph: { title: doc.title, description: doc.description, images: [] },
      twitter: {
        card: 'summary',
        title: doc.title,
        description: doc.description,
        images: [],
      },
    };
  }
  return { title: doc.title, description: doc.description };
}

export default async function DocumentationPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  const doc = getDoc(slug);
  if (!doc) notFound();
  if (doc.slug === 'privacy-security-comparison') {
    return (
      <SecurityComparisonShell doc={doc}>
        <div dangerouslySetInnerHTML={{ __html: doc.html }} />
      </SecurityComparisonShell>
    );
  }
  return (
    <DocsShell doc={doc}>
      <div dangerouslySetInnerHTML={{ __html: doc.html }} />
    </DocsShell>
  );
}
