# Operations Pulse

Build a polished, responsive incident command center using only `index.html`,
`styles.css`, and `app.js`; no framework and no external assets.

On load, fetch `https://benchmark.invalid/api/incidents`. Each incident has
`id`, `title`, `service`, `severity` (`critical|high|medium|low`), `status`
(`open|resolved`), and ISO `updated_at` fields.

Requirements:

- Show a clear loading state, useful empty state, and retryable error state.
- Render incident cards with service, severity, status, and a human-readable
  update time. Deduplicate by id, keeping the newest valid record, and sort newest
  first by default without mutating API data.
- Provide a labelled search, severity select, `All`/`Open`/`Resolved` status
  controls, and newest/oldest sort. Combine filters and show a result count.
- Persist search, severity, status, and sort in URL query parameters; browser
  reload/back navigation must restore the view. Omit default values from the URL.
- Each card has an accessible favorite toggle persisted in localStorage.
- Open incidents have a Resolve button. PATCH
  `https://benchmark.invalid/api/incidents/ID` with JSON `{ "status": "resolved" }`.
  Disable it while pending. On failure, keep the incident open and announce a
  useful alert; on success update the view and announce success.
- Include visible keyboard focus, WCAG 2 AA contrast, semantic landmarks and
  headings, an `aria-live` status region, and no horizontal overflow at 390px.

Stable hooks: `incident-list`, `incident-card`, `result-count`, `search`,
`severity`, `sort`, `favorite-ID`, `resolve-ID`, plus `data-status="all|open|resolved"`.

Do not change the tests or configuration. Run `npm test` until every test passes.
