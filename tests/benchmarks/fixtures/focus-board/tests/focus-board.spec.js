const { test, expect } = require("@playwright/test");
const AxeBuilder = require("@axe-core/playwright").default;
const path = require("path");
const { pathToFileURL } = require("url");

const pageUrl = pathToFileURL(path.join(__dirname, "..", "index.html")).href;

test.beforeEach(async ({ page }) => {
  await page.goto(pageUrl);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
});

test("adds, completes, filters, and persists tasks", async ({ page }) => {
  await expect(page.getByRole("heading", { name: "Focus Board", exact: true })).toBeVisible();
  await expect(page.getByLabel("Task", { exact: true })).toBeVisible();

  const input = page.getByTestId("task-input");
  await input.fill("Ship benchmark report");
  await page.getByRole("button", { name: "Add task" }).click();
  await input.fill("Review accessibility");
  await page.getByRole("button", { name: "Add task" }).click();

  await expect(page.getByTestId("open-list").getByTestId("task-item")).toHaveCount(2);
  await expect(page.getByTestId("remaining-count")).toContainText("2");

  await page.getByTestId("open-list").getByRole("checkbox").first().check();
  await expect(page.getByTestId("done-list").getByTestId("task-item")).toHaveCount(1);
  await expect(page.getByTestId("remaining-count")).toContainText("1");

  await page.locator('[data-filter="open"]').click();
  await expect(page.getByTestId("done-list")).toBeHidden();
  await page.reload();
  await expect(page.getByText("Ship benchmark report", { exact: true })).toBeVisible();
  await expect(page.getByTestId("remaining-count")).toContainText("1");
});

test("is accessible and does not overflow on mobile", async ({ page }) => {
  const accessibility = await new AxeBuilder({ page }).analyze();
  expect(accessibility.violations).toEqual([]);

  await page.setViewportSize({ width: 390, height: 844 });
  const dimensions = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth);
});
