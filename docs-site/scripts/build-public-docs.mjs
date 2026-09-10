import { mkdir, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { siteDocuments } from './generate-docs.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = path.join(root, 'out');
const base = '/s-code-docs';
const origin = 'https://sl-7qx.github.io';
const escape = (value) => value.replaceAll('&', '&amp;').replaceAll('<', '&lt;')
  .replaceAll('>', '&gt;').replaceAll('"', '&quot;');
const route = (slug) => `${base}/docs/${slug}/`;
const groups = [...new Set(siteDocuments.map((doc) => doc.group))];

function page(doc) {
  const navigation = groups.map((group) => `<section><h2>${escape(group)}</h2>${siteDocuments
    .filter((entry) => entry.group === group)
    .map((entry) => `<a href="${route(entry.slug)}"${entry.slug === doc.slug ? ' aria-current="page"' : ''}>${escape(entry.shortTitle)}</a>`)
    .join('')}</section>`).join('');
  const index = siteDocuments.findIndex((entry) => entry.slug === doc.slug);
  const adjacent = [siteDocuments[index - 1], siteDocuments[index + 1]];
  const body = doc.html.replaceAll('href="/docs/', `href="${base}/docs/`);
  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escape(doc.title)} · S-Code Docs</title><meta name="description" content="${escape(doc.description)}">
<link rel="canonical" href="${origin}${route(doc.slug)}"><link rel="stylesheet" href="${base}/style.css">
<script src="${base}/site.js" defer></script></head><body>
<a class="skip" href="#content">Skip to content</a>
<header><a class="brand" href="${base}/">S-Code <span>DOCS</span></a><div class="header-actions"><a href="${route('quick-start')}">Get started</a><button id="theme" type="button" aria-label="Switch color theme">Theme</button></div></header>
<div class="layout"><aside class="sidebar"><details class="navigation" open><summary>Documentation</summary>
<label for="search">Search documentation</label><input id="search" type="search" placeholder="Search topics…" autocomplete="off"><ul id="results" aria-label="Search results" aria-live="polite" hidden></ul><nav aria-label="Documentation">${navigation}</nav></details></aside>
<main id="content"><div class="eyebrow">${escape(doc.group)}</div><h1>${escape(doc.title)}</h1><p class="lead">${escape(doc.description)}</p>
<article class="doc-content">${body}</article><nav class="adjacent" aria-label="Adjacent pages">${adjacent.map((entry, i) => entry ? `<a href="${route(entry.slug)}"><small>${i ? 'Next' : 'Previous'}</small>${escape(entry.shortTitle)}</a>` : '<span></span>').join('')}</nav><footer>S-Code · Apache 2.0 · Developer Preview</footer></main>
<aside class="toc"><nav aria-label="On this page"><h2>On this page</h2>${doc.headings.map((heading) => `<a href="#${escape(heading.id)}">${escape(heading.label)}</a>`).join('')}</nav></aside></div></body></html>`;
}

await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
for (const doc of siteDocuments) {
  const directory = path.join(output, 'docs', doc.slug);
  await mkdir(directory, { recursive: true });
  await writeFile(path.join(directory, 'index.html'), page(doc));
}
await writeFile(path.join(output, 'index.html'), page(siteDocuments.find((doc) => doc.slug === 'introduction') ?? siteDocuments[0]));
await writeFile(path.join(output, '.nojekyll'), '');
await writeFile(path.join(output, '404.html'), `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Page not found · S-Code</title><link rel="stylesheet" href="${base}/style.css"><main><h1>Page not found</h1><p><a href="${base}/">Return to S-Code documentation</a></p></main></html>`);
await writeFile(path.join(output, 'search.json'), JSON.stringify(siteDocuments.map(({ slug, title, description, keywords }) => ({ href: route(slug), title, description, keywords }))));
await writeFile(path.join(output, 'style.css'), `
:root{color-scheme:light;--bg:#fff;--panel:#f5f7f8;--ink:#142027;--muted:#54636c;--line:#dce4e8;--accent:#086659;--code:#10262a}
@media(prefers-color-scheme:dark){:root:not([data-theme=light]){color-scheme:dark;--bg:#11191e;--panel:#182329;--ink:#e7eff2;--muted:#a3b5be;--line:#304149;--accent:#76dbc4;--code:#071419}}
:root[data-theme=dark]{color-scheme:dark;--bg:#11191e;--panel:#182329;--ink:#e7eff2;--muted:#a3b5be;--line:#304149;--accent:#76dbc4;--code:#071419}
*{box-sizing:border-box}html{scroll-behavior:smooth;scroll-padding-top:6rem}body{margin:0;background:var(--bg);color:var(--ink);font:1rem/1.75 system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}a{color:var(--accent);text-underline-offset:.2em}a:hover{text-decoration:underline}button,input{font:inherit;color:inherit;background:var(--bg);border:1px solid var(--line);border-radius:.4rem}button{padding:.3rem .8rem;cursor:pointer}:focus-visible{outline:3px solid var(--accent);outline-offset:3px}
header{height:4.5rem;border-bottom:1px solid var(--line);display:flex;align-items:center;justify-content:space-between;padding:0 2rem;position:sticky;top:0;background:var(--bg);z-index:2}.brand{font-size:1.35rem;font-weight:750;color:var(--ink);text-decoration:none}.brand span{font-size:.75rem;letter-spacing:.12em;color:var(--muted);margin-left:.6rem}.header-actions{display:flex;align-items:center;gap:1.25rem;font-size:.875rem}.layout{display:grid;grid-template-columns:16rem minmax(0,1fr) 13rem;max-width:100rem;margin:auto}.sidebar{border-right:1px solid var(--line);padding:2rem 1.25rem;position:sticky;top:4.5rem;max-height:calc(100vh - 4.5rem);overflow:auto;align-self:start}.navigation summary{font-weight:650;cursor:pointer;margin-bottom:1.25rem}.sidebar label{display:block;font-size:.875rem;color:var(--muted);margin-bottom:.4rem}input{width:100%;padding:.55rem .65rem}.sidebar section{margin-top:1.7rem}.sidebar h2,.toc h2{font-size:.875rem;margin:0 0 .6rem;color:var(--ink)}.sidebar nav a,.toc a{display:block;text-decoration:none;font-size:.875rem;color:var(--muted);padding:.3rem .6rem;margin:.1rem 0}.sidebar a[aria-current]{background:var(--panel);color:var(--accent);border-left:3px solid var(--accent);font-weight:650}.sidebar #results{padding:0;list-style:none;font-size:.875rem}.sidebar #results a{display:block;padding:.45rem 0}.sidebar #results p{font-size:.875rem;margin:0;color:var(--muted)}
main{padding:3.5rem clamp(1.25rem,4vw,4rem);min-width:0;max-width:65rem}.eyebrow{color:var(--accent);font-size:.875rem;font-weight:650;letter-spacing:.08em;text-transform:uppercase}h1{font-size:clamp(2rem,4vw,3.4rem);line-height:1.12;letter-spacing:-.045em;margin:.8rem 0 1.1rem;overflow-wrap:anywhere}.lead{font-size:1.15rem;color:var(--muted);margin-bottom:2.8rem;max-width:50rem}.doc-content h2{border-top:1px solid var(--line);margin-top:2.5rem;padding-top:1.7rem;font-size:1.55rem;line-height:1.3;letter-spacing:-.02em}.doc-content h3{font-size:1.15rem;margin-top:1.8rem;line-height:1.4}.doc-content p,.doc-content li{overflow-wrap:anywhere}.doc-content li{margin:.45rem 0}.doc-content code{font-size:.875em;background:var(--panel);padding:.15rem .35rem;border-radius:.2rem}.doc-content pre{padding:1.2rem;background:var(--code);color:#dcf2ee;overflow:auto;border-radius:.5rem;line-height:1.65}.doc-content pre code{background:none;padding:0;font-size:.875rem}.doc-content blockquote{margin:1.5rem 0;padding:.1rem 1.25rem;border-left:3px solid var(--accent);background:var(--panel);color:var(--muted)}.doc-table-wrap{overflow:auto;border:1px solid var(--line);border-radius:.35rem;margin:1.5rem 0}table{border-collapse:collapse;width:100%;font-size:.875rem}th,td{padding:.7rem .85rem;border-bottom:1px solid var(--line);text-align:left;vertical-align:top;min-width:8rem}th{background:var(--panel);font-weight:650}.toc{padding:3.5rem 1rem;position:sticky;top:4.5rem;align-self:start;max-height:calc(100vh - 4.5rem);overflow:auto}.toc a{border-left:1px solid var(--line);line-height:1.45;padding:.5rem .75rem}.adjacent{display:grid;grid-template-columns:1fr 1fr;gap:1rem;margin-top:3rem}.adjacent a{border:1px solid var(--line);border-radius:.4rem;padding:1rem;text-decoration:none}.adjacent small{display:block;color:var(--muted);font-size:.875rem}.adjacent a:last-child{text-align:right}footer{border-top:1px solid var(--line);margin-top:2rem;padding-top:1.5rem;color:var(--muted);font-size:.875rem}.skip{position:fixed;left:1rem;top:-5rem;z-index:4;background:var(--bg);padding:.5rem 1rem}.skip:focus{top:.5rem}
@media(max-width:1150px){.layout{grid-template-columns:15rem minmax(0,1fr)}.toc{display:none}}@media(max-width:760px){header{padding:0 1rem}.layout{display:block}.sidebar{position:static;max-height:none;border-right:0;border-bottom:1px solid var(--line);padding:1rem}.navigation:not([open]) summary{margin:0}main{padding:2rem 1.25rem}.adjacent{grid-template-columns:1fr}.header-actions{gap:.75rem}}@media(prefers-reduced-motion:reduce){html{scroll-behavior:auto}}
`);
await writeFile(path.join(output, 'site.js'), `
const root=document.documentElement;
try{const theme=localStorage.getItem('s-code-docs-theme');if(theme==='dark'||theme==='light')root.dataset.theme=theme}catch{}
document.getElementById('theme').addEventListener('click',()=>{const dark=root.dataset.theme?root.dataset.theme==='dark':matchMedia('(prefers-color-scheme:dark)').matches;root.dataset.theme=dark?'light':'dark';try{localStorage.setItem('s-code-docs-theme',root.dataset.theme)}catch{}});
if(matchMedia('(max-width:760px)').matches)document.querySelector('.navigation').open=false;
const input=document.getElementById('search'),results=document.getElementById('results');let entries;
input.addEventListener('input',async()=>{const query=input.value.trim().toLowerCase();results.hidden=!query;if(!query){results.replaceChildren();return}try{entries??=fetch('${base}/search.json').then(response=>{if(!response.ok)throw Error();return response.json()});const docs=await entries;if(query!==input.value.trim().toLowerCase())return;const matches=docs.filter(doc=>[doc.title,doc.description,...doc.keywords].join(' ').toLowerCase().includes(query));results.replaceChildren();for(const doc of matches){const item=document.createElement('li'),link=document.createElement('a');link.href=doc.href;link.textContent=doc.title;item.append(link);results.append(item)}if(!matches.length){const item=document.createElement('li');item.textContent='No matching pages';results.append(item)}}catch{entries=undefined;results.textContent='Search is unavailable. Browse topics below.'}});
`);
console.log(`Built ${siteDocuments.length} public documentation pages in out/`);
