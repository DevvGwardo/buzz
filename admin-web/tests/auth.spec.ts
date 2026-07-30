import { expect, type Page, test } from "@playwright/test";

const TOKEN =
  "5f0e1d2c3b4a59687786958493a2b1c0decadebeefcafe0123456789abcdef01";
const STORAGE_KEY = "buzz-admin-token";

async function seedToken(page: Page, token = TOKEN) {
  await page.addInitScript(
    ([key, value]) => {
      sessionStorage.setItem(key, value);
    },
    [STORAGE_KEY, token],
  );
}

/// Records the Authorization header of every admin API call the SPA makes.
async function recordAuthorization(page: Page, body: unknown = []) {
  const seen: (string | undefined)[] = [];
  await page.route("**/api/admin/v1/**", async (route) => {
    seen.push(route.request().headers().authorization);
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify(body),
    });
  });
  return seen;
}

test("the dashboard prompts for a token before any api call", async ({
  page,
}) => {
  const seen = await recordAuthorization(page);
  await page.goto("/reports");
  await expect(
    page.getByRole("heading", { name: "Admin token required" }),
  ).toBeVisible();
  expect(seen).toHaveLength(0);

  await page.getByPlaceholder("Admin token").fill(TOKEN);
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(
    page.getByRole("heading", { name: "Open reports" }),
  ).toBeVisible();
  expect(seen).toContain(`Bearer ${TOKEN}`);
});

test("api calls carry the stored token", async ({ page }) => {
  await seedToken(page);
  const seen = await recordAuthorization(page);
  await page.goto("/reports");
  await expect(
    page.getByRole("heading", { name: "Open reports" }),
  ).toBeVisible();
  expect(seen).toEqual([`Bearer ${TOKEN}`]);
});

test("the token survives a reload within the session", async ({ page }) => {
  await page.route("**/api/admin/v1/**", (route) =>
    route.fulfill({ contentType: "application/json", body: "[]" }),
  );
  await page.goto("/reports");
  await page.getByPlaceholder("Admin token").fill(TOKEN);
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(
    page.getByRole("heading", { name: "Open reports" }),
  ).toBeVisible();

  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Open reports" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Admin token required" }),
  ).toHaveCount(0);
});

test("a rejected token clears storage and re-prompts once", async ({
  page,
}) => {
  await seedToken(page, "f".repeat(64));
  await page.route("**/api/admin/v1/**", (route) =>
    route.fulfill({
      status: 401,
      contentType: "application/json",
      body: JSON.stringify({
        error: { code: "unauthorized", message: "rejected" },
      }),
    }),
  );

  await page.goto("/reports");
  const prompt = page.getByRole("heading", { name: "Admin token required" });
  await expect(prompt).toHaveCount(1);
  await expect(page.getByText("That token was rejected.")).toBeVisible();
  expect(
    await page.evaluate((key) => sessionStorage.getItem(key), STORAGE_KEY),
  ).toBeNull();
});

test("attachments are fetched with the token and rendered from blob urls", async ({
  page,
}) => {
  const id = "feedback-with-attachments";
  const imageHash = "a".repeat(64);
  const fileHash = "b".repeat(64);
  const imageUrl = `https://design.buzz.xyz/media/${imageHash}.png`;
  const fileUrl = `https://design.buzz.xyz/media/${fileHash}.txt`;
  await seedToken(page);

  const attachmentRequests: { path: string; authorization?: string }[] = [];
  await page.route(`**/api/admin/v1/feedback/${id}/attachments/**`, (route) => {
    attachmentRequests.push({
      path: new URL(route.request().url()).pathname,
      authorization: route.request().headers().authorization,
    });
    route.fulfill({ contentType: "application/octet-stream", body: "bytes" });
  });
  await page.route(`**/api/admin/v1/feedback/${id}`, (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        id,
        communityId: "one",
        communityHost: "design.buzz.xyz",
        eventId: "31".repeat(32),
        submitterPubkey: "21".repeat(32),
        category: "bug",
        body: "Composer froze.",
        tags: [
          [
            "imeta",
            `url ${imageUrl}`,
            "m image/png",
            `x ${imageHash}`,
            "filename screenshot.png",
          ],
          [
            "imeta",
            `url ${fileUrl}`,
            "m text/plain",
            `x ${fileHash}`,
            "filename diagnostics.txt",
          ],
        ],
        eventCreatedAt: "2026-07-17T17:25:00Z",
        receivedAt: "2026-07-17T17:30:00Z",
      }),
    }),
  );

  await page.goto(`/feedback/${id}`);
  await expect(
    page.getByRole("img", { name: "screenshot.png" }),
  ).toHaveAttribute("src", /^blob:/);
  await expect(
    page.getByRole("link", { name: /diagnostics.txt/ }),
  ).toHaveAttribute("href", /^blob:/);

  expect(attachmentRequests.map((request) => request.path).sort()).toEqual(
    [
      `/api/admin/v1/feedback/${id}/attachments/${imageHash}`,
      `/api/admin/v1/feedback/${id}/attachments/${fileHash}`,
    ].sort(),
  );
  for (const request of attachmentRequests) {
    expect(request.authorization).toBe(`Bearer ${TOKEN}`);
  }
});
