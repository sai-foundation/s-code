import { access, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import matter from 'gray-matter';
import { marked } from 'marked';
import sanitizeHtml from 'sanitize-html';

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const siteRoot = path.resolve(scriptDirectory, '..');
const canonicalRoot = path.resolve(siteRoot, '..', 'docs');
const contentRoot = path.join(siteRoot, 'content');
const generatedModule = path.join(siteRoot, 'lib', 'generated-docs.ts');
const checkOnly = process.argv.includes('--check');
const repositoryUrl = 'https://github.com/sai-foundation/s-code/blob/main';

async function exists(candidate) {
  try {
    await access(candidate);
    return true;
  } catch {
    return false;
  }
}

async function listMarkdownFiles(root) {
  const files = [];

  async function visit(directory) {
    const entries = (await readdir(directory, { withFileTypes: true })).sort((left, right) =>
      left.name.localeCompare(right.name),
    );
    for (const entry of entries) {
      const candidate = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) {
        throw new Error(`documentation sources cannot be symlinks: ${candidate}`);
      }
      if (entry.isDirectory()) {
        await visit(candidate);
      } else if (entry.isFile() && entry.name.endsWith('.md')) {
        files.push(candidate);
      }
    }
  }

  await visit(root);
  return files;
}

function requireString(metadata, field, source) {
  const value = metadata[field];
  if (typeof value !== 'string' || !value.trim()) {
    throw new Error(`${source}: front matter field ${field} must be a non-empty string`);
  }
  return value.trim();
}

function headingLabel(markdown) {
  return markdown
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/[`*_~]/g, '')
    .trim();
}

function headingId(label, counts) {
  const base = label
    .normalize('NFKD')
    .toLowerCase()
    .replace(/[^\p{Letter}\p{Number}]+/gu, '-')
    .replace(/^-+|-+$/g, '') || 'section';
  const count = (counts.get(base) ?? 0) + 1;
  counts.set(base, count);
  return count === 1 ? base : `${base}-${count}`;
}

function escapeHtml(value) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function prepareBody(markdown, source, title) {
  const lines = markdown.replace(/\r\n/g, '\n').split('\n');
  const firstContent = lines.findIndex((line) => line.trim());
  if (firstContent < 0 || !lines[firstContent].startsWith('# ')) {
    throw new Error(`${source}: documentation page must start with an H1`);
  }
  const h1 = headingLabel(lines[firstContent].slice(2));
  if (h1 !== title) {
    throw new Error(`${source}: H1 ${JSON.stringify(h1)} must match title ${JSON.stringify(title)}`);
  }
  lines.splice(firstContent, 1);

  const counts = new Map();
  const headings = [];
  const prepared = lines.map((line) => {
    const match = /^(#{2,3})\s+(.+?)\s*$/.exec(line);
    if (!match) return line;
    const label = headingLabel(match[2]);
    const id = headingId(label, counts);
    if (match[1].length === 2) headings.push({ id, label });
    return `<h${match[1].length} id="${escapeHtml(id)}">${escapeHtml(label)}</h${match[1].length}>`;
  });

  if (headings.length === 0) {
    throw new Error(`${source}: documentation page must contain at least one H2`);
  }
  return { markdown: prepared.join('\n').trim(), headings };
}

function rewriteHref(href, sourceRelative, routeBySource) {
  if (/^(?:https?:|mailto:|#)/.test(href)) return href;
  const [pathname, fragment = ''] = href.split('#', 2);
  if (!pathname) return fragment ? `#${fragment}` : href;

  const resolved = path.posix.normalize(path.posix.join(path.posix.dirname(sourceRelative), pathname));
  if (pathname.endsWith('.md') && routeBySource.has(resolved)) {
    const suffix = fragment ? `#${fragment}` : '';
    return `/docs/${routeBySource.get(resolved)}${suffix}`;
  }

  const repositoryPath = path.posix.normalize(path.posix.join('docs', path.posix.dirname(sourceRelative), pathname));
  if (repositoryPath.startsWith('../') || repositoryPath === '..') {
    throw new Error(`${sourceRelative}: link escapes the Community repository: ${href}`);
  }
  const suffix = fragment ? `#${fragment}` : '';
  return `${repositoryUrl}/${repositoryPath}${suffix}`;
}

function renderMarkdown(markdown, sourceRelative, routeBySource) {
  let html = marked.parse(markdown, { gfm: true });
  html = html.replace(/\shref="([^"]+)"/g, (_match, href) =>
    ` href="${escapeHtml(rewriteHref(href, sourceRelative, routeBySource))}"`,
  );
  html = html.replaceAll('<table>', '<div class="doc-table-wrap"><table>');
  html = html.replaceAll('</table>', '</table></div>');
  return sanitizeHtml(html, {
    allowedTags: [
      'a', 'blockquote', 'br', 'code', 'div', 'em', 'h2', 'h3', 'hr', 'kbd', 'li',
      'ol', 'p', 'pre', 'strong', 'table', 'tbody', 'td', 'th', 'thead', 'tr', 'ul',
    ],
    allowedAttributes: {
      a: ['href', 'title'],
      code: ['class'],
      div: ['class'],
      h2: ['id'],
      h3: ['id'],
    },
    allowedClasses: {
      code: [/^language-[a-z0-9_-]+$/],
      div: ['doc-table-wrap'],
    },
    allowedSchemes: ['http', 'https', 'mailto'],
    allowProtocolRelative: false,
  });
}

async function readEntries(sourceRoot) {
  const files = await listMarkdownFiles(sourceRoot);
  const entries = [];
  for (const file of files) {
    const raw = await readFile(file, 'utf8');
    const parsed = matter(raw);
    if (parsed.data.site !== true) continue;
    const sourceRelative = path.relative(sourceRoot, file).split(path.sep).join('/');
    const slug = requireString(parsed.data, 'slug', sourceRelative);
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(slug)) {
      throw new Error(`${sourceRelative}: slug must contain lowercase letters, digits and single hyphens`);
    }
    const order = parsed.data.order;
    if (!Number.isInteger(order) || order < 0) {
      throw new Error(`${sourceRelative}: front matter field order must be a non-negative integer`);
    }
    if (!Array.isArray(parsed.data.keywords) || !parsed.data.keywords.every((value) => typeof value === 'string')) {
      throw new Error(`${sourceRelative}: front matter field keywords must be a string array`);
    }
    entries.push({
      sourceRelative,
      raw,
      slug,
      order,
      title: requireString(parsed.data, 'title', sourceRelative),
      shortTitle: requireString(parsed.data, 'short_title', sourceRelative),
      group: requireString(parsed.data, 'group', sourceRelative),
      description: requireString(parsed.data, 'description', sourceRelative),
      keywords: parsed.data.keywords.map((value) => value.trim()).filter(Boolean),
      body: parsed.content,
    });
  }

  const slugs = new Set();
  const orders = new Set();
  for (const entry of entries) {
    if (slugs.has(entry.slug)) throw new Error(`duplicate documentation slug: ${entry.slug}`);
    if (orders.has(entry.order)) throw new Error(`duplicate documentation order: ${entry.order}`);
    slugs.add(entry.slug);
    orders.add(entry.order);
  }
  return entries.sort((left, right) => left.order - right.order);
}

async function syncContent(entries) {
  const expected = new Set(entries.map((entry) => entry.sourceRelative));
  const current = (await exists(contentRoot)) ? await listMarkdownFiles(contentRoot) : [];
  const stale = current.filter((file) => !expected.has(path.relative(contentRoot, file).split(path.sep).join('/')));
  const mismatches = [];

  for (const entry of entries) {
    const destination = path.join(contentRoot, ...entry.sourceRelative.split('/'));
    const existing = (await exists(destination)) ? await readFile(destination, 'utf8') : null;
    if (existing !== entry.raw) {
      if (checkOnly) {
        mismatches.push(entry.sourceRelative);
      } else {
        await mkdir(path.dirname(destination), { recursive: true });
        await writeFile(destination, entry.raw, 'utf8');
      }
    }
  }

  if (checkOnly && stale.length > 0) {
    mismatches.push(...stale.map((file) => path.relative(contentRoot, file).split(path.sep).join('/')));
  } else if (!checkOnly) {
    for (const file of stale) await rm(file);
  }

  if (mismatches.length > 0) {
    const sortedMismatches = [...new Set(mismatches)].sort((left, right) => left.localeCompare(right));
    throw new Error(`generated documentation mirror is stale: ${sortedMismatches.join(', ')}`);
  }
}

function renderEntries(entries) {
  const routeBySource = new Map(entries.map((entry) => [entry.sourceRelative, entry.slug]));
  return entries.map((entry) => {
    const prepared = prepareBody(entry.body, entry.sourceRelative, entry.title);
    return {
      slug: entry.slug,
      title: entry.title,
      shortTitle: entry.shortTitle,
      group: entry.group,
      description: entry.description,
      keywords: entry.keywords,
      sourcePath: `docs/${entry.sourceRelative}`,
      headings: prepared.headings,
      html: renderMarkdown(prepared.markdown, entry.sourceRelative, routeBySource),
    };
  });

}

function moduleSource(entries) {
  const rendered = renderEntries(entries);
  return `// Generated by scripts/generate-docs.mjs. Do not edit.\n\n` +
    `export type DocEntry = {\n` +
    `  slug: string;\n  title: string;\n  shortTitle: string;\n  group: string;\n` +
    `  description: string;\n  keywords: string[];\n  sourcePath: string;\n` +
    `  headings: { id: string; label: string }[];\n  html: string;\n};\n\n` +
    `export const docs: DocEntry[] = ${JSON.stringify(rendered, null, 2)};\n\n` +
    `export const docGroups = [...new Set(docs.map((doc) => doc.group))];\n\n` +
    `export function getDoc(slug: string) {\n  return docs.find((doc) => doc.slug === slug);\n}\n\n` +
    `export function getAdjacentDocs(slug: string) {\n` +
    `  const index = docs.findIndex((doc) => doc.slug === slug);\n` +
    `  return {\n    previous: index > 0 ? docs[index - 1] : undefined,\n` +
    `    next: index >= 0 && index < docs.length - 1 ? docs[index + 1] : undefined,\n  };\n}\n`;
}

const canonicalAvailable = await exists(canonicalRoot);
const sourceRoot = canonicalAvailable ? canonicalRoot : contentRoot;
if (!(await exists(sourceRoot))) {
  throw new Error('no canonical or mirrored documentation source is available');
}

const entries = await readEntries(sourceRoot);
if (entries.length === 0) throw new Error('no site-enabled Community documentation pages found');
if (canonicalAvailable) await syncContent(entries);

if (!checkOnly) {
  await mkdir(path.dirname(generatedModule), { recursive: true });
  await writeFile(generatedModule, moduleSource(entries), 'utf8');
}

process.stdout.write(`${checkOnly ? 'validated' : 'generated'} ${entries.length} Community documentation pages\n`);

export const siteDocuments = renderEntries(entries);
