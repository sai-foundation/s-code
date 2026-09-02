# Focus Board

Build a polished, responsive single-page task board using only `index.html`,
`styles.css`, and `app.js`—no framework and no external assets.

The page needs:

- a clear `Focus Board` heading and a short product introduction;
- an accessible form with a visible `Task` label, text input, and `Add task`
  button;
- `All`, `Open`, and `Done` filters;
- open and completed task sections;
- a checkbox for every task and a visible remaining-task count;
- local persistence so tasks and completion state survive a reload;
- a deliberate visual system with responsive layout, strong focus states, and
  no horizontal overflow at 390px viewport width.

Use these stable hooks without changing their meaning:

- `data-testid="task-form"`, `task-input`, and `remaining-count`;
- `data-filter="all|open|done"` on filter buttons;
- `data-testid="open-list"` and `done-list` on the two task lists;
- `data-testid="task-item"` on each rendered task.

Do not change the tests. Run `npm test` and keep working until every test
passes.
