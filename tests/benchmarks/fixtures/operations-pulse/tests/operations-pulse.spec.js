const { test, expect } = require("@playwright/test");
const AxeBuilder = require("@axe-core/playwright").default;
const path = require("path");
const { pathToFileURL } = require("url");

const pageUrl = pathToFileURL(path.join(__dirname, "..", "index.html")).href;
const incidents = [
  { id: "api-1", title: "Elevated API latency", service: "Payments", severity: "high", status: "open", updated_at: "2026-08-30T12:00:00Z" },
  { id: "cache-1", title: "Cache saturation", service: "Catalog", severity: "critical", status: "open", updated_at: "2026-08-30T13:00:00Z" },
  { id: "api-1", title: "Stale duplicate", service: "Payments", severity: "low", status: "open", updated_at: "2026-08-29T12:00:00Z" },
  { id: "deploy-1", title: "Deployment rollback", service: "Checkout", severity: "medium", status: "resolved", updated_at: "2026-08-30T11:00:00Z" },
  { id: "search-1", title: "Search indexing lag", service: "Discovery", severity: "low", status: "open", updated_at: "2026-08-30T10:00:00Z" },
];

async function installApi(page) {
  await page.route("https://benchmark.invalid/api/incidents", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify(incidents),
  }));
  await page.route("https://benchmark.invalid/api/incidents/*", route => route.fulfill({
    status: 200, contentType: "application/json", body: '{"status":"resolved"}',
  }));
}

test.beforeEach(async ({ page }) => {
  await installApi(page);
  await page.goto(pageUrl);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
});

test("loads, deduplicates, filters, sorts, and restores URL state", async ({ page }) => {
  await expect(page.getByRole("heading", { name: "Operations Pulse", exact: true })).toBeVisible();
  await expect(page.getByTestId("incident-card")).toHaveCount(4);
  await expect(page.getByTestId("incident-card").first()).toContainText("Cache saturation");
  await expect(page.getByTestId("result-count")).toContainText("4");

  await page.getByTestId("search").fill("payments");
  await expect(page.getByTestId("incident-card")).toHaveCount(1);
  await page.getByTestId("severity").selectOption("high");
  await page.locator('[data-status="open"]').click();
  await page.getByTestId("sort").selectOption("oldest");
  await expect.poll(() => new URL(page.url()).searchParams.get("q")).toBe("payments");
  await expect.poll(() => new URL(page.url()).searchParams.get("severity")).toBe("high");
  await page.reload();
  await expect(page.getByTestId("search")).toHaveValue("payments");
  await expect(page.getByTestId("severity")).toHaveValue("high");
  await expect(page.locator('[data-status="open"]')).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("sort")).toHaveValue("oldest");
});

test("favorites persist and resolve rolls back on error then succeeds", async ({ page }) => {
  let attempts = 0;
  await page.unroute("https://benchmark.invalid/api/incidents/*");
  await page.route("https://benchmark.invalid/api/incidents/*", async route => {
    attempts += 1;
    const request = route.request();
    expect(request.method()).toBe("PATCH");
    expect(request.postDataJSON()).toEqual({ status: "resolved" });
    if (attempts === 1) await route.fulfill({ status: 503, body: "unavailable" });
    else await route.fulfill({ status: 200, contentType: "application/json", body: '{"status":"resolved"}' });
  });

  await page.getByTestId("favorite-api-1").click();
  await expect(page.getByTestId("favorite-api-1")).toHaveAttribute("aria-pressed", "true");
  await page.reload();
  await expect(page.getByTestId("favorite-api-1")).toHaveAttribute("aria-pressed", "true");

  await page.getByTestId("resolve-api-1").click();
  await expect(page.getByRole("alert")).toContainText(/try|failed|unavailable/i);
  await expect(page.getByTestId("resolve-api-1")).toBeEnabled();
  await page.getByTestId("resolve-api-1").click();
  await expect(page.getByTestId("resolve-api-1")).toHaveCount(0);
  await expect(page.getByText(/resolved/i).last()).toBeVisible();
});

test("shows retryable fetch errors and recovers", async ({ page }) => {
  await page.unroute("https://benchmark.invalid/api/incidents");
  let requests = 0;
  await page.route("https://benchmark.invalid/api/incidents", async route => {
    requests += 1;
    if (requests === 1) await route.fulfill({ status: 500, body: "no" });
    else await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(incidents) });
  });
  await page.reload();
  await expect(page.getByRole("alert")).toBeVisible();
  await page.getByRole("button", { name: /retry/i }).click();
  await expect(page.getByTestId("incident-card")).toHaveCount(4);
});

test("is accessible and does not overflow on mobile", async ({ page }) => {
  await expect(page.getByTestId("incident-card")).toHaveCount(4);
  const accessibility = await new AxeBuilder({ page }).analyze();
  expect(accessibility.violations).toEqual([]);
  await page.setViewportSize({ width: 390, height: 844 });
  const dimensions = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth);
});
