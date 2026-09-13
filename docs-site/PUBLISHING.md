# Public documentation publishing

The public documentation is hosted at <https://sl-7qx.github.io/s-code-docs/>.
GitHub Pages serves the `gh-pages` branch of `sl-7qx/s-code-docs`. That public
repository contains generated documentation assets only. Authoring stays in
this repository, with source changes reviewed through pull requests.

The canonical Markdown in `docs/` supplies both the public static site and the
React documentation site. `npm run build:pages --prefix docs-site` generates
the static site in `docs-site/out/`; the existing documentation CI also checks
this build. The static output has no server or login dependency. It includes
navigation, page search, color themes and responsive layouts.

After a reviewed source change, rebuild and copy the contents of `out/` into
a clean checkout of the public documentation repository's `gh-pages` branch.
Commit the generated files with a DCO sign-off, push, and wait for the GitHub
Pages deployment to finish. Keep `.nojekyll` in the output. Verify the homepage
and a nested document without authentication before sharing the link.

The Sites configuration is retained for the optional React deployment. Its
replacement publication failed during TLS provisioning; it is not the public
documentation entrypoint.
