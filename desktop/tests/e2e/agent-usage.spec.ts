import { expect, test, type Page } from "@playwright/test";

import {
  installMockBridge,
  TEST_IDENTITIES,
  type MockAgentUsage,
  type MockAgentUsageSeries,
} from "../helpers/bridge";

/**
 * A non-owned, non-managed agent (declared owner is `outsider`, not the mock
 * viewer) — used by the A13 fail-closed tests where eligibility must come
 * from archived evidence alone, not ownership.
 */
const HISTORICAL_AGENT_PUBKEY = "6".repeat(64);

function getHashSearchParam(page: Page, name: string) {
  const hash = new URL(page.url()).hash.replace(/^#/, "");
  const queryStart = hash.indexOf("?");
  if (queryStart === -1) {
    return null;
  }
  return new URLSearchParams(hash.slice(queryStart + 1)).get(name);
}

async function expectHashSearchParam(
  page: Page,
  name: string,
  value: string | null,
) {
  await expect.poll(() => getHashSearchParam(page, name)).toBe(value);
}

function usageField(value: string | null, incomplete = false) {
  return { value, incomplete };
}

function costField(value: number | null, incomplete = false) {
  return { value, incomplete };
}

function reportedUsage(
  overrides: Partial<{
    inputTokens: string | null;
    outputTokens: string | null;
    totalTokens: string | null;
    estimatedCostUsd: number | null;
  }> = {},
) {
  return {
    estimatedCostUsd: costField(overrides.estimatedCostUsd ?? null),
    inputTokens: usageField(overrides.inputTokens ?? null),
    outputTokens: usageField(overrides.outputTokens ?? null),
    totalTokens: usageField(overrides.totalTokens ?? null),
  };
}

function mockAgentUsage(
  agentPubkey: string,
  overrides: Partial<MockAgentUsage> = {},
): MockAgentUsage {
  return {
    agentPubkey,
    buckets: [],
    hasUnknownUsage: false,
    models: [],
    reportCount: 1,
    // Real-world shape: no publisher emits totalTokens today; i/o are always
    // present. Seeding this as the default ensures the suite exercises the
    // "approx ≈" display path that prod users see, not just the happy-path
    // seed that previously masked the "No usage reported" regression.
    usage: reportedUsage({ inputTokens: "1200", outputTokens: "300" }),
    ...overrides,
  };
}

function mockUsageSeries(
  overrides: Partial<MockAgentUsageSeries> = {},
): MockAgentUsageSeries {
  return {
    agents: [],
    buckets: [],
    collectionEnabled: true,
    coverage: {
      firstArchivedAt: null,
      firstReportedAt: null,
      hasUnknownUsage: false,
      invalidReportCount: 0,
      lastArchivedAt: null,
      lastReportedAt: null,
      reportCount: 0,
    },
    hasArchivedEvidence: null,
    ...overrides,
  };
}

async function addGenericAgent(
  page: Page,
  channelName: string,
  agentName: string,
): Promise<string> {
  await page.getByTestId(`channel-${channelName}`).click();
  await expect(page.getByTestId("chat-title")).toHaveText(channelName);
  const channelId = await page
    .getByTestId(`channel-${channelName}`)
    .getAttribute("data-channel-id");
  if (!channelId) {
    throw new Error(`Channel ${channelName} is missing a data-channel-id.`);
  }

  await page.waitForFunction(() => {
    return Boolean(
      (window as Window & { __BUZZ_E2E_INVOKE_MOCK_COMMAND__?: unknown })
        .__BUZZ_E2E_INVOKE_MOCK_COMMAND__,
    );
  });
  return page.evaluate(
    async ({ agentName, channelId }): Promise<string> => {
      const invoke = (
        window as Window & {
          __BUZZ_E2E_INVOKE_MOCK_COMMAND__?: (
            command: string,
            payload?: Record<string, unknown>,
          ) => Promise<{ agent?: { pubkey: string } }>;
        }
      ).__BUZZ_E2E_INVOKE_MOCK_COMMAND__;
      if (!invoke) {
        throw new Error("Mock bridge is not installed.");
      }

      const created = (await invoke("create_managed_agent", {
        input: {
          name: agentName,
          spawnAfterCreate: true,
          systemPrompt: "Watch the channel and help when asked.",
        },
      })) as { agent?: { pubkey: string } };
      const pubkey = created.agent?.pubkey;
      if (!pubkey) {
        throw new Error("Mock managed agent creation did not return a pubkey.");
      }

      await invoke("add_channel_members", {
        channelId,
        pubkeys: [pubkey],
        role: "bot",
      });

      await (
        window as Window & {
          __BUZZ_E2E_QUERY_CLIENT__?: {
            invalidateQueries: () => Promise<void>;
          };
        }
      ).__BUZZ_E2E_QUERY_CLIENT__?.invalidateQueries();

      return pubkey;
    },
    { agentName, channelId },
  );
}

async function openAgentsView(page: Page) {
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agents-usage-section")).toBeVisible({
    timeout: 10_000,
  });
}

test("shows a loading skeleton while the usage series is in flight", async ({
  page,
}) => {
  await installMockBridge(page, {
    agentUsageSeries: mockUsageSeries(),
    agentUsageDelayMs: 2_000,
  });

  await openAgentsView(page);

  await expect(page.getByTestId("agent-usage-skeleton")).toBeVisible();
  await expect(page.getByTestId("agent-usage-card")).toBeVisible({
    timeout: 5_000,
  });
  await expect(page.getByTestId("agent-usage-skeleton")).toHaveCount(0);
});

test("renders ranked agent rows and switches between the 1d, 7d, and 30d presets", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Token Bot");

  await page.evaluate(
    (series) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    mockUsageSeries({
      agents: [
        mockAgentUsage(agentPubkey, {
          usage: reportedUsage({
            inputTokens: "1200",
            outputTokens: "300",
            totalTokens: "1500",
          }),
        }),
      ],
      coverage: {
        firstArchivedAt: 1_700_000_000,
        firstReportedAt: 1_700_000_000,
        hasUnknownUsage: false,
        invalidReportCount: 0,
        lastArchivedAt: 1_700_086_400,
        lastReportedAt: 1_700_086_400,
        reportCount: 1,
      },
    }),
  );
  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agents-usage-section")).toBeVisible();

  const row = page.getByTestId(`agent-usage-row-${agentPubkey}`);
  await expect(row).toBeVisible();
  await expect(row).toContainText("1.5K");
  await expect(row).toContainText("Token Bot");

  await expect(page.getByTestId("agent-usage-window-7")).toHaveAttribute(
    "data-state",
    "active",
  );

  for (const preset of ["30", "1"]) {
    await page.getByTestId(`agent-usage-window-${preset}`).click();
    await expect(
      page.getByTestId(`agent-usage-window-${preset}`),
    ).toHaveAttribute("data-state", "active");
    // Switching presets must never blank the card — the query re-runs against
    // a new boundary set but the seeded series is returned for every window.
    await expect(page.getByTestId("agent-usage-card")).toBeVisible();
  }
});

test("clicking an agent row opens the profile panel's Usage focused view", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Drilldown Bot");
  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [
          mockAgentUsage(agentPubkey, {
            models: [
              {
                harness: "goose",
                hasUnknownUsage: false,
                model: "claude-opus",
                reportCount: 1,
                usage: reportedUsage({ totalTokens: "1500" }),
              },
            ],
            usage: reportedUsage({
              estimatedCostUsd: 0.42,
              inputTokens: "1200",
              outputTokens: "300",
              totalTokens: "1500",
            }),
          }),
        ],
      }),
    },
  );
  await page.getByTestId("open-agents-view").click();
  await expect(
    page.getByTestId(`agent-usage-row-${agentPubkey}`),
  ).toBeVisible();

  await page.getByTestId(`agent-usage-row-${agentPubkey}`).click();
  await expect(page.getByTestId("user-profile-panel")).toBeVisible();
  await expect(page.getByTestId("agent-usage-focused-view")).toBeVisible();
  await expect(page.getByTestId("agent-usage-focused-totals")).toContainText(
    "1,500",
  );
  await expect(page.getByTestId("agent-usage-focused-models")).toContainText(
    "claude-opus",
  );
  await expect(page.getByTestId("agent-usage-focused-models")).toContainText(
    "goose",
  );

  // Reaching the same view via the Info-tab ingress row lands on the same
  // subview (covers the ProfileIngressRow entry point, not just the
  // row-click shortcut).
  await page.getByTestId("user-profile-panel-back").click();
  await expect(page.getByTestId("user-profile-tab-info")).toBeVisible();
  await page.getByTestId(`user-profile-view-usage-${agentPubkey}`).click();
  await expect(page.getByTestId("agent-usage-focused-view")).toBeVisible();
});

// A non-owned, non-managed agent seeded purely via `searchProfiles` (no
// managed-agent creation) — used by the ingress-visibility test's
// non-owner leg, which must be hidden regardless of archived evidence.
const NON_OWNER_AGENT_PUBKEY = "7".repeat(64);

test("Info-tab Usage ingress is visible for an owner-viewed agent, and absent for a human or a non-owner agent", async ({
  page,
}) => {
  await installMockBridge(page, {
    agentUsageSeries: mockUsageSeries(),
    searchProfiles: [
      {
        pubkey: NON_OWNER_AGENT_PUBKEY,
        displayName: "Someone Else's Bot",
        isAgent: true,
        ownerPubkey: TEST_IDENTITIES.outsider.pubkey,
      },
    ],
  });

  // Owner case: a locally-managed agent is owned by the mock viewer by
  // construction, so `canViewUsage` (viewerIsOwner && isBot) is true.
  await openAgentsView(page);
  const ownedAgentPubkey = await addGenericAgent(page, "general", "Own Bot");
  await page.evaluate(
    (series) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    mockUsageSeries({ agents: [mockAgentUsage(ownedAgentPubkey)] }),
  );
  await page.getByTestId("open-agents-view").click();
  await page.getByTestId(`agent-usage-row-${ownedAgentPubkey}`).click();
  await expect(page.getByTestId("user-profile-panel")).toBeVisible();
  await page.getByTestId("user-profile-panel-back").click();
  await expect(page.getByTestId("user-profile-tab-info")).toBeVisible();
  await expect(
    page.getByTestId(`user-profile-view-usage-${ownedAgentPubkey}`),
  ).toBeVisible();

  // Human case: `canViewUsage` requires `isBot`, so a plain human profile
  // never renders the row, regardless of ownership.
  await page.evaluate((pubkey) => {
    (
      window as Window & {
        __TSR_ROUTER__?: { navigate: (opts: Record<string, unknown>) => void };
      }
    ).__TSR_ROUTER__?.navigate({
      to: "/agents",
      search: { profile: pubkey },
    });
  }, TEST_IDENTITIES.bob.pubkey);
  await expect(page.getByTestId("user-profile-panel")).toBeVisible();
  await expect(
    page.getByTestId(`user-profile-view-usage-${TEST_IDENTITIES.bob.pubkey}`),
  ).toHaveCount(0);

  // Non-owner agent case: `isBot` is true but `viewerIsOwner` is false (the
  // declared owner is `outsider`, not the mock viewer) — the row stays
  // hidden even though the agent is eligible for the focused view via
  // archived evidence (A13 fail-closed is a separate axis from ingress
  // visibility).
  await page.evaluate((pubkey) => {
    (
      window as Window & {
        __TSR_ROUTER__?: { navigate: (opts: Record<string, unknown>) => void };
      }
    ).__TSR_ROUTER__?.navigate({
      to: "/agents",
      search: { profile: pubkey },
    });
  }, NON_OWNER_AGENT_PUBKEY);
  await expect(page.getByTestId("user-profile-panel")).toBeVisible();
  await expect(
    page.getByTestId(`user-profile-view-usage-${NON_OWNER_AGENT_PUBKEY}`),
  ).toHaveCount(0);
});

test("the usage card fits a compact viewport without horizontal overflow, and its controls stay keyboard-focusable", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Compact Bot");
  await page.evaluate(
    (series) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    mockUsageSeries({
      agents: [mockAgentUsage(agentPubkey)],
      buckets: Array.from({ length: 8 }, (_, i) => ({
        start: 1_700_000_000 + i * 86_400,
        end: 1_700_000_000 + (i + 1) * 86_400,
        usage: reportedUsage({ totalTokens: String((i + 1) * 100) }),
        reportCount: 1,
        hasUnknownUsage: false,
      })),
    }),
  );
  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agent-usage-card")).toBeVisible();

  await page.setViewportSize({ width: 520, height: 900 });
  await expect(page.getByTestId("agent-usage-card")).toBeVisible();

  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth,
  );
  expect(overflow).toBe(false);

  // Window-selector tabs and the agent row remain keyboard-focusable at
  // this width — Tab forward from the 7d control must reach 30d, and the
  // row must remain reachable and clickable via focus + Enter.
  const window7 = page.getByTestId("agent-usage-window-7");
  const window30 = page.getByTestId("agent-usage-window-30");
  await window7.focus();
  await expect(window7).toBeFocused();
  // Radix's `Tabs` uses a roving tabindex: only the active tab is in the
  // Tab order, and arrow keys move focus (and selection) between tabs.
  await page.keyboard.press("ArrowRight");
  await expect(window30).toBeFocused();

  const row = page.getByTestId(`agent-usage-row-${agentPubkey}`);
  await row.focus();
  await expect(row).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("agent-usage-focused-view")).toBeVisible();
});

test("surfaces a retry affordance when the usage query fails, and recovers on retry", async ({
  page,
}) => {
  // React Query's global `retry: 1` (queryClient.ts) auto-retries once
  // before the UI ever sees an error, silently consuming one entry of the
  // sequence — so two failures are needed before the UI's own error state
  // (and its Retry button) appears; the third entry is the button click.
  await installMockBridge(page, {
    agentUsageErrors: ["archive unavailable", "archive unavailable", null],
  });

  await openAgentsView(page);

  const error = page.getByTestId("agent-usage-error");
  await expect(error).toBeVisible();
  await expect(error).toContainText("Couldn't load usage data.");

  await error.getByRole("button", { name: "Retry" }).click();

  await expect(page.getByTestId("agent-usage-card")).toBeVisible();
  await expect(page.getByTestId("agent-usage-error")).toHaveCount(0);
});

test("shows the empty state when collection is on but nothing has been archived yet", async ({
  page,
}) => {
  await installMockBridge(page, {
    agentUsageSeries: mockUsageSeries(),
  });

  await openAgentsView(page);

  const empty = page.getByTestId("agent-usage-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("No locally archived usage");
  await expect(page.getByTestId("agent-usage-collection-off")).toHaveCount(0);
});

test("shows the collection-off banner with a settings deep link, with and without retained data", async ({
  page,
}) => {
  await installMockBridge(page, {
    agentUsageSeries: mockUsageSeries({ collectionEnabled: false }),
  });

  await openAgentsView(page);

  const banner = page.getByTestId("agent-usage-collection-off");
  await expect(banner).toBeVisible();
  await expect(banner).toContainText("Local usage collection is off.");
  await expect(page.getByTestId("agent-usage-empty")).toContainText(
    "Turn on collection",
  );

  await banner
    .getByRole("button", { name: "Open Local Archive settings" })
    .click();
  await expect(page.getByTestId("settings-view")).toBeVisible();
  await expect(page.getByTestId("settings-local-archive")).toBeVisible({
    timeout: 10_000,
  });

  await page.goBack();
  await expect(page.getByTestId("agents-usage-section")).toBeVisible();
});

test("shows retained-data coverage copy when collection is off but usage was previously archived", async ({
  page,
}) => {
  await installMockBridge(page, {
    agentUsageSeries: mockUsageSeries({
      collectionEnabled: false,
      coverage: {
        firstArchivedAt: 1_700_000_000,
        firstReportedAt: 1_700_000_000,
        hasUnknownUsage: false,
        invalidReportCount: 0,
        lastArchivedAt: 1_700_086_400,
        lastReportedAt: 1_700_086_400,
        reportCount: 3,
      },
    }),
  });

  await openAgentsView(page);

  await expect(page.getByTestId("agent-usage-collection-off")).toContainText(
    "Collection off · data through",
  );
});

// A13 fail-closed contract (Rev 3): the focused view's eligibility is
// ownership OR archived evidence, decided only once the author-filtered
// query resolves. Both tests below deep-link straight to
// `?profileView=usage` for a non-owned, non-managed agent (declared owner is
// `outsider`, not the mock viewer) so neither test depends on ownership.
async function openUsageViewForHistoricalAgent(
  page: Page,
  agentUsageSeries: MockAgentUsageSeries,
) {
  await installMockBridge(page, {
    agentUsageSeries,
    searchProfiles: [
      {
        pubkey: HISTORICAL_AGENT_PUBKEY,
        displayName: "Historical Bot",
        isAgent: true,
        ownerPubkey: TEST_IDENTITIES.outsider.pubkey,
      },
    ],
  });
  await page.goto("/#/agents", { waitUntil: "domcontentloaded" });
  await expect(page.getByTestId("agents-usage-section")).toBeVisible({
    timeout: 10_000,
  });
  await page.evaluate((pubkey) => {
    (
      window as Window & {
        __TSR_ROUTER__?: {
          navigate: (opts: Record<string, unknown>) => void;
        };
      }
    ).__TSR_ROUTER__?.navigate({
      to: "/agents",
      search: { profile: pubkey, profileView: "usage" },
    });
  }, HISTORICAL_AGENT_PUBKEY);
  await expect(page.getByTestId("user-profile-panel")).toBeVisible({
    timeout: 10_000,
  });
}

test("a historical author with 30d-only archived evidence gets a valid empty 7d focused view, not a redirect", async ({
  page,
}) => {
  await openUsageViewForHistoricalAgent(
    page,
    mockUsageSeries({ agents: [], hasArchivedEvidence: true }),
  );

  const outsideWindow = page.getByTestId("agent-usage-focused-outside-window");
  await expect(outsideWindow).toBeVisible();
  await expect(outsideWindow).toContainText("Try a wider window.");

  // Eligible via archived evidence alone (no ownership) — never bounced.
  await expectHashSearchParam(page, "profileView", "usage");
  const panel = page.getByTestId("user-profile-panel");
  await expect(
    panel.getByRole("heading", { level: 2, name: "Usage" }),
  ).toBeVisible();
});

test("a hand-authored usage URL with no ownership and no archived evidence falls back to summary", async ({
  page,
}) => {
  await openUsageViewForHistoricalAgent(
    page,
    mockUsageSeries({ agents: [], hasArchivedEvidence: null }),
  );

  // The redirect from "usage" → null may fire before the first poll
  // observes profileView=usage — skip that transient assertion; the null
  // landing + summary heading carry the contract.
  await expectHashSearchParam(page, "profileView", null);
  await expect(page.getByTestId("agent-usage-focused-view")).toHaveCount(0);
  const panel = page.getByTestId("user-profile-panel");
  await expect(
    panel.getByRole("heading", { level: 2, name: "Profile" }),
  ).toBeVisible();
});

test("renders a Partial badge for an agent whose total is a known lower bound", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Partial Bot");
  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [
          mockAgentUsage(agentPubkey, {
            hasUnknownUsage: true,
            usage: {
              estimatedCostUsd: costField(null),
              inputTokens: usageField(null),
              outputTokens: usageField(null),
              totalTokens: usageField("900", true),
            },
          }),
        ],
      }),
    },
  );
  await page.getByTestId("open-agents-view").click();

  const row = page.getByTestId(`agent-usage-row-${agentPubkey}`);
  await expect(row).toBeVisible();
  await expect(row.getByText("Partial", { exact: true })).toBeVisible();
});

// ── F4: daily bars behavioral coverage ───────────────────────────────────────

test("overview card renders daily bars with correct accessible labels distinguishing known, unknown, and empty days", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Bar Bot");

  // Three top-level buckets: one known (100 tokens), one unknown (reports but
  // null total), one empty (zero reports).  The daily bar component must never
  // encode "unknown" as "zero" — confirmed via aria-label inspection.
  const knownStart = 1_700_000_000;
  const unknownStart = knownStart + 86_400;
  const emptyStart = unknownStart + 86_400;

  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [mockAgentUsage(agentPubkey, { buckets: [] })],
        buckets: [
          {
            start: knownStart,
            end: knownStart + 86_400,
            usage: reportedUsage({ totalTokens: "100" }),
            reportCount: 1,
            hasUnknownUsage: false,
          },
          {
            start: unknownStart,
            end: unknownStart + 86_400,
            // reportCount > 0 but totalTokens.value null → unknown, not zero
            usage: reportedUsage({ totalTokens: null }),
            reportCount: 1,
            hasUnknownUsage: true,
          },
          {
            start: emptyStart,
            end: emptyStart + 86_400,
            usage: reportedUsage({ totalTokens: null }),
            reportCount: 0,
            hasUnknownUsage: false,
          },
        ],
      }),
    },
  );

  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agent-usage-overall-bars")).toBeVisible();

  const knownBar = page.getByTestId(`agent-usage-daily-bar-${knownStart}`);
  const unknownBar = page.getByTestId(`agent-usage-daily-bar-${unknownStart}`);
  const emptyBar = page.getByTestId(`agent-usage-daily-bar-${emptyStart}`);

  await expect(knownBar).toBeVisible();
  await expect(unknownBar).toBeVisible();
  await expect(emptyBar).toBeVisible();

  // Unknown day must have a distinct label — NOT "no usage reported" and NOT
  // a token count.  Known and empty must each carry their own label too.
  const knownLabel = await knownBar
    .locator("[aria-label]")
    .first()
    .getAttribute("aria-label");
  const unknownLabel = await unknownBar
    .locator("[aria-label]")
    .first()
    .getAttribute("aria-label");
  const emptyLabel = await emptyBar
    .locator("[aria-label]")
    .first()
    .getAttribute("aria-label");

  expect(knownLabel).toMatch(/reported tokens/i);
  expect(unknownLabel).toMatch(/unknown usage/i);
  expect(emptyLabel).toMatch(/no usage reported/i);

  // Verify the three labels are all distinct — unknown must not collapse to zero.
  expect(unknownLabel).not.toBe(emptyLabel);
  expect(unknownLabel).not.toBe(knownLabel);
});

test("focused view shows daily bars, coverage dates, and a partial explanation when usage is incomplete", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Coverage Bot");

  const bucketStart = 1_700_000_000;
  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [
          mockAgentUsage(agentPubkey, {
            hasUnknownUsage: true,
            reportCount: 3,
            buckets: [
              {
                start: bucketStart,
                end: bucketStart + 86_400,
                usage: reportedUsage({
                  // Incomplete i/o — proves the "input/output usage could not
                  // be counted" caveat sentence and triggers the gate.
                  inputTokens: "400",
                  outputTokens: "100",
                  totalTokens: "500",
                }),
                reportCount: 2,
                hasUnknownUsage: true,
              },
            ],
            usage: {
              // Incomplete inputTokens → showUnknownIntervalsCaveat fires.
              estimatedCostUsd: costField(null),
              inputTokens: usageField("400", true), // incomplete
              outputTokens: usageField("100", false),
              totalTokens: usageField("500"),
            },
          }),
        ],
        coverage: {
          firstArchivedAt: 1_700_000_000,
          firstReportedAt: 1_700_000_000,
          hasUnknownUsage: true,
          invalidReportCount: 1,
          lastArchivedAt: 1_700_086_400,
          lastReportedAt: 1_700_086_400,
          reportCount: 3,
        },
      }),
    },
  );

  // Navigate to the focused view via the row click shortcut.
  await page.getByTestId("open-agents-view").click();
  await expect(
    page.getByTestId(`agent-usage-row-${agentPubkey}`),
  ).toBeVisible();
  await page.getByTestId(`agent-usage-row-${agentPubkey}`).click();
  await expect(page.getByTestId("agent-usage-focused-view")).toBeVisible();

  // Daily bars section must appear in the focused view.
  await expect(
    page.getByTestId("agent-usage-focused-daily-bars"),
  ).toBeVisible();

  // Coverage section must contain the report count.
  const coverage = page.getByTestId("agent-usage-focused-coverage");
  await expect(coverage).toBeVisible();
  await expect(coverage).toContainText("reported turn");

  // Both caveat sentences must appear when the gate conditions are met.
  // The seed has inputTokens.incomplete=true (fires unknown-intervals sentence)
  // AND invalidReportCount=1 (fires invalid-reports sentence).
  await expect(
    page.getByTestId("agent-usage-focused-unknown-intervals-caveat"),
  ).toBeVisible();
  await expect(
    page.getByTestId("agent-usage-focused-invalid-reports-caveat"),
  ).toBeVisible();
});

// ── T1: invalid-only window behavioral coverage ───────────────────────────────

test("overview and focused view distinguish invalid-only windows from ordinary empty windows", async ({
  page,
}) => {
  // An invalid-only window: invalidReportCount > 0, zero valid agents/buckets.
  // The overview must NOT say "No locally archived usage in the last N days"
  // and the focused view must NOT show the outside-window hint — both would
  // mislabel in-window-but-uncountable evidence as absent.
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Invalid Bot");

  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [], // no valid rows
        buckets: [], // invalid rows never bucketed
        coverage: {
          firstArchivedAt: 1_700_000_000,
          firstReportedAt: null,
          hasUnknownUsage: true,
          invalidReportCount: 2, // the signal
          lastArchivedAt: 1_700_086_400,
          lastReportedAt: null,
          reportCount: 0,
        },
        hasArchivedEvidence: true, // A13 returns true for invalid rows too
      }),
    },
  );

  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agent-usage-card")).toBeVisible();

  // Overview empty-state must reflect uncountable usage, not ordinary empty.
  const empty = page.getByTestId("agent-usage-empty");
  await expect(empty).toBeVisible();
  await expect(empty).not.toContainText("No locally archived usage");
  await expect(empty).toContainText("could not be counted");

  // Navigate directly to the focused usage view for this agent.
  await page.evaluate((pubkey) => {
    (
      window as Window & {
        __TSR_ROUTER__?: { navigate: (opts: Record<string, unknown>) => void };
      }
    ).__TSR_ROUTER__?.navigate({
      to: "/agents",
      search: { profile: pubkey, profileView: "usage" },
    });
  }, agentPubkey);
  await expect(page.getByTestId("agent-usage-focused-view")).toBeVisible({
    timeout: 10_000,
  });

  // Focused view must show the invalid-only state, NOT the outside-window hint.
  await expect(
    page.getByTestId("agent-usage-focused-invalid-only"),
  ).toBeVisible();
  await expect(
    page.getByTestId("agent-usage-focused-outside-window"),
  ).toHaveCount(0);
});

// ── T1b: Behavioral regression guard — null totalTokens with known i/o renders ≈ ─
//
// This scenario will fail if the approximate display fallback is removed.
// It covers the real-world prod shape where no publisher emits totalTokens
// today — the overview row, header, daily bar, and focused total must all
// visibly render ≈ rather than "No usage reported".

test("overview row, header, daily bar, and focused total all render ≈ when totalTokens is null but i/o is known", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Approx Bot");

  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [
          mockAgentUsage(agentPubkey, {
            buckets: [
              {
                start: 1_700_000_000,
                end: 1_700_086_400,
                hasUnknownUsage: false,
                reportCount: 1,
                usage: reportedUsage({
                  inputTokens: "800",
                  outputTokens: "200",
                  totalTokens: null, // real prod shape — no provider total
                }),
              },
            ],
            usage: reportedUsage({
              inputTokens: "800",
              outputTokens: "200",
              totalTokens: null, // real prod shape — no provider total
            }),
          }),
        ],
        buckets: [
          {
            start: 1_700_000_000,
            end: 1_700_086_400,
            hasUnknownUsage: false,
            reportCount: 1,
            usage: reportedUsage({
              inputTokens: "800",
              outputTokens: "200",
              totalTokens: null,
            }),
          },
        ],
        coverage: {
          firstArchivedAt: 1_700_000_000,
          firstReportedAt: 1_700_000_000,
          hasUnknownUsage: false,
          invalidReportCount: 0,
          lastArchivedAt: 1_700_086_400,
          lastReportedAt: 1_700_086_400,
          reportCount: 1,
        },
      }),
    },
  );

  await page.getByTestId("open-agents-view").click();

  // Overview row must render ≈ approximate total, not "No usage reported".
  const row = page.getByTestId(`agent-usage-row-${agentPubkey}`);
  await expect(row).toBeVisible();
  await expect(row).toContainText("≈");
  await expect(row).not.toContainText("No usage reported");

  // Section header must render ≈ total (sumKnownBucketTotals → approximate).
  // Assert the dedicated value node directly so the header fails independently
  // even if a child daily bar still shows ≈.
  const headerValue = page.getByTestId("agent-usage-header-value");
  await expect(headerValue).toBeVisible();
  await expect(headerValue).toContainText("≈");
  await expect(headerValue).not.toContainText("No usage reported");

  // Daily bar must render with a bar (not a hatched unknown baseline) and
  // show ≈ trailing label.
  const dailyBars = page.getByTestId("agent-usage-daily-bars");
  await expect(dailyBars).toBeVisible();
  await expect(dailyBars).toContainText("≈");

  // Focused total must render ≈ approximate stat.
  // Assert the dedicated value node directly so the stat fails independently
  // even if a focused daily bar still shows ≈.
  await row.click();
  await expect(page.getByTestId("user-profile-panel")).toBeVisible();
  await expect(page.getByTestId("agent-usage-focused-view")).toBeVisible();
  const focusedTotalValue = page.getByTestId("agent-usage-focused-total-value");
  await expect(focusedTotalValue).toBeVisible();
  await expect(focusedTotalValue).toContainText("≈");
  await expect(focusedTotalValue).not.toHaveText("—");
});

// ── T2: I/O-incomplete partial behavioral coverage ────────────────────────────

test("overview row shows Partial badge and ingress shows partial marker when I/O fields are incomplete with null total", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "IO Partial Bot");

  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [
          mockAgentUsage(agentPubkey, {
            buckets: [
              {
                start: 1_700_000_000,
                end: 1_700_086_400,
                hasUnknownUsage: false,
                reportCount: 1,
                usage: {
                  // null total, incomplete input — approx-partial bar kind
                  estimatedCostUsd: costField(null),
                  inputTokens: usageField("800", true), // incomplete
                  outputTokens: usageField("200", false),
                  totalTokens: usageField(null),
                },
              },
            ],
            usage: {
              // null total, incomplete I/O — per-field Partial must surface
              estimatedCostUsd: costField(null),
              inputTokens: usageField("800", true), // incomplete
              outputTokens: usageField("200", false),
              totalTokens: usageField(null),
            },
          }),
        ],
        buckets: [
          {
            start: 1_700_000_000,
            end: 1_700_086_400,
            hasUnknownUsage: false,
            reportCount: 1,
            usage: {
              // null total, incomplete input — approx-partial aggregate
              estimatedCostUsd: costField(null),
              inputTokens: usageField("800", true), // incomplete
              outputTokens: usageField("200", false),
              totalTokens: usageField(null),
            },
          },
        ],
      }),
    },
  );

  await page.getByTestId("open-agents-view").click();
  const row = page.getByTestId(`agent-usage-row-${agentPubkey}`);
  await expect(row).toBeVisible();

  // Row must show Partial badge — the approximate total carries partial provenance.
  await expect(row.getByText("Partial", { exact: true })).toBeVisible();
  // Row must show the ≈ approximate total (both i/o present, null total → approx display).
  await expect(row).toContainText("≈ 1K");

  // Daily bar for an approx-partial bucket must use ≈N* trailing text
  // (distinct from plain ≈N so a sighted user sees the partial signal).
  const dailyBars = page.getByTestId("agent-usage-daily-bars");
  await expect(dailyBars).toBeVisible();
  await expect(dailyBars).toContainText("≈1K*");

  // Open the profile panel to reach the Info tab for ingress verification.
  await row.click();
  await expect(page.getByTestId("user-profile-panel")).toBeVisible();
  await page.getByTestId("user-profile-panel-back").click();
  await expect(page.getByTestId("user-profile-tab-info")).toBeVisible();

  // The ingress trailing for an I/O-only-with-partial series must contain
  // "Partial" (i.e., "Input/output reported · Partial").
  const ingressRow = page.getByTestId(`user-profile-view-usage-${agentPubkey}`);
  await expect(ingressRow).toBeVisible();
  await expect(ingressRow).toContainText("Partial");
});

// ── Task C: date x-axis, on-bar values, hover tooltip ────────────────────────

test("each daily bar labels its date on the x-axis and its token total on the bar", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Axis Bot");

  const knownStart = 1_700_000_000;
  const unknownStart = knownStart + 86_400;

  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [mockAgentUsage(agentPubkey, { buckets: [] })],
        buckets: [
          {
            start: knownStart,
            end: knownStart + 86_400,
            usage: reportedUsage({
              inputTokens: "1000",
              outputTokens: "500",
              totalTokens: "1500",
            }),
            reportCount: 1,
            hasUnknownUsage: false,
          },
          {
            start: unknownStart,
            end: unknownStart + 86_400,
            usage: reportedUsage({ totalTokens: null }),
            reportCount: 1,
            hasUnknownUsage: true,
          },
        ],
      }),
    },
  );

  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agent-usage-overall-bars")).toBeVisible();

  // The x-axis tick is the bucket's DATE, rendered in the browser's locale —
  // asserted against the same `toLocaleDateString` call the component makes,
  // so the expectation is timezone/locale-independent.
  const expectedDate = await page.evaluate(
    (start) =>
      new Date(start * 1000).toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
      }),
    knownStart,
  );
  const dateTick = page.getByTestId(`agent-usage-daily-bar-date-${knownStart}`);
  await expect(dateTick).toBeVisible();
  await expect(dateTick).toHaveText(expectedDate);
  // The date must NOT be the token total — the pre-fix chart labeled the axis
  // with the value, which is the bug this asserts against.
  await expect(dateTick).not.toContainText("1.5K");

  // The token total renders directly ON the bar.
  const valueLabel = page.getByTestId(
    `agent-usage-daily-bar-value-${knownStart}`,
  );
  await expect(valueLabel).toBeVisible();
  await expect(valueLabel).toHaveText("1.5K");

  // An uncountable day shows an em-dash on the bar, never a zero.
  const unknownValue = page.getByTestId(
    `agent-usage-daily-bar-value-${unknownStart}`,
  );
  await expect(unknownValue).toHaveText("—");
});

test("hovering a daily bar reveals a tooltip with the total, input, and output breakdown", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Tooltip Bot");

  const bucketStart = 1_700_000_000;
  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [mockAgentUsage(agentPubkey, { buckets: [] })],
        buckets: [
          {
            start: bucketStart,
            end: bucketStart + 86_400,
            usage: reportedUsage({
              inputTokens: "1200",
              outputTokens: "300",
              totalTokens: "1500",
            }),
            reportCount: 1,
            hasUnknownUsage: false,
          },
        ],
      }),
    },
  );

  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agent-usage-overall-bars")).toBeVisible();

  await page.getByTestId(`agent-usage-daily-bar-${bucketStart}`).hover();

  // Exact grouped values — the split must be legible without opening the
  // focused view, which is the whole point of the tooltip.
  const tooltip = page
    .getByTestId(`agent-usage-daily-bar-tooltip-${bucketStart}`)
    .first();
  await expect(tooltip).toBeVisible({ timeout: 10_000 });
  await expect(tooltip).toContainText("Total: 1,500");
  await expect(tooltip).toContainText("Input: 1,200");
  await expect(tooltip).toContainText("Output: 300");
});

test("a bar tooltip reports an unknown field as unknown rather than zero", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Halfknown Bot");

  const bucketStart = 1_700_000_000;
  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    {
      series: mockUsageSeries({
        agents: [mockAgentUsage(agentPubkey, { buckets: [] })],
        buckets: [
          {
            start: bucketStart,
            end: bucketStart + 86_400,
            // Output is genuinely unreported: it must read "unknown", and the
            // total must stay unknown rather than being derived from input.
            usage: reportedUsage({
              inputTokens: "1200",
              outputTokens: null,
              totalTokens: null,
            }),
            reportCount: 1,
            hasUnknownUsage: true,
          },
        ],
      }),
    },
  );

  await page.getByTestId("open-agents-view").click();
  await expect(page.getByTestId("agent-usage-overall-bars")).toBeVisible();

  await page.getByTestId(`agent-usage-daily-bar-${bucketStart}`).hover();

  const tooltip = page
    .getByTestId(`agent-usage-daily-bar-tooltip-${bucketStart}`)
    .first();
  await expect(tooltip).toBeVisible({ timeout: 10_000 });
  await expect(tooltip).toContainText("Total: unknown");
  await expect(tooltip).toContainText("Input: 1,200");
  await expect(tooltip).toContainText("Output: unknown");
  await expect(tooltip).not.toContainText("Output: 0");
});

// ── Task D: custom date-range picker ─────────────────────────────────────────

test("the custom range picker applies an arbitrary date span and labels it in the empty state", async ({
  page,
}) => {
  await installMockBridge(page, { agentUsageSeries: mockUsageSeries() });
  await openAgentsView(page);

  await page.getByTestId("agent-usage-window-custom").click();
  await expect(
    page.getByTestId("agent-usage-window-custom-popover"),
  ).toBeVisible();

  await page.getByTestId("agent-usage-window-custom-start").fill("2026-01-05");
  await page.getByTestId("agent-usage-window-custom-end").fill("2026-01-09");
  await expect(
    page.getByTestId("agent-usage-window-custom-summary"),
  ).toContainText("5 days selected");

  await page.getByTestId("agent-usage-window-custom-apply").click();
  await expect(
    page.getByTestId("agent-usage-window-custom-popover"),
  ).toHaveCount(0);

  // The custom tab is now the active window, and the empty-state copy names
  // the applied span instead of "the last N days".
  await expect(page.getByTestId("agent-usage-window-custom")).toHaveAttribute(
    "data-state",
    "active",
  );
  const empty = page.getByTestId("agent-usage-empty");
  await expect(empty).toBeVisible();
  await expect(empty).toContainText("2026");
  await expect(empty).not.toContainText("the last 7 days");
});

test("the custom range picker blocks an inverted range and a span over one year", async ({
  page,
}) => {
  await installMockBridge(page, { agentUsageSeries: mockUsageSeries() });
  await openAgentsView(page);

  await page.getByTestId("agent-usage-window-custom").click();
  const apply = page.getByTestId("agent-usage-window-custom-apply");
  const error = page.getByTestId("agent-usage-window-custom-error");

  await page.getByTestId("agent-usage-window-custom-start").fill("2026-03-10");
  await page.getByTestId("agent-usage-window-custom-end").fill("2026-03-01");
  await expect(error).toContainText("on or before");
  await expect(apply).toBeDisabled();

  // 2024-01-01 → 2025-01-01 is 367 civil days: one past the cap the backend's
  // boundary arity admits, so the picker must refuse it locally rather than
  // letting the request surface a Rust error.
  await page.getByTestId("agent-usage-window-custom-start").fill("2024-01-01");
  await page.getByTestId("agent-usage-window-custom-end").fill("2025-01-01");
  await expect(error).toContainText("366 days or fewer");
  await expect(apply).toBeDisabled();

  // Pulling the end back to the cap clears the error and enables Apply.
  await page.getByTestId("agent-usage-window-custom-end").fill("2024-12-31");
  await expect(
    page.getByTestId("agent-usage-window-custom-summary"),
  ).toContainText("366 days selected");
  await expect(apply).toBeEnabled();
});

test("the focused view exposes its own independent range selector", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const agentPubkey = await addGenericAgent(page, "general", "Focused Bot");

  await page.evaluate(
    ({ series }) => {
      const testWindow = window as Window & {
        __BUZZ_E2E__?: { mock?: { agentUsageSeries?: unknown } };
      };
      testWindow.__BUZZ_E2E__ ??= {};
      testWindow.__BUZZ_E2E__.mock ??= {};
      testWindow.__BUZZ_E2E__.mock.agentUsageSeries = series;
    },
    { series: mockUsageSeries({ agents: [mockAgentUsage(agentPubkey)] }) },
  );

  await page.getByTestId("open-agents-view").click();
  await page.getByTestId(`agent-usage-row-${agentPubkey}`).click();
  await expect(page.getByTestId("agent-usage-focused-view")).toBeVisible();

  await page.getByTestId("agent-usage-focused-window-1").click();
  await expect(
    page.getByTestId("agent-usage-focused-window-1"),
  ).toHaveAttribute("data-state", "active");

  // The overview's own selector is unaffected — the two windows are
  // deliberately independent.
  await expect(page.getByTestId("agent-usage-window-7")).toHaveAttribute(
    "data-state",
    "active",
  );
});

// ── Nav order: the Usage section sits at the bottom of the Agents page ───────

test("the usage section renders below the agents and teams sections", async ({
  page,
}) => {
  await installMockBridge(page);
  await openAgentsView(page);

  const order = await page.evaluate(() => {
    const testIds = [
      "agents-library-personas",
      "agents-library-teams",
      "agents-usage-section",
    ];
    return testIds.map((testId) => {
      const element = document.querySelector(`[data-testid="${testId}"]`);
      return element === null
        ? null
        : element.getBoundingClientRect().top + window.scrollY;
    });
  });

  const [personasTop, teamsTop, usageTop] = order;
  expect(personasTop).not.toBeNull();
  expect(teamsTop).not.toBeNull();
  expect(usageTop).not.toBeNull();
  expect(usageTop as number).toBeGreaterThan(personasTop as number);
  expect(usageTop as number).toBeGreaterThan(teamsTop as number);
});
