import { expect, test, type Page } from "@playwright/test";

import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const SELF_PUBKEY = "deadbeef".repeat(8);
const CHANNEL_GENERAL = "9a1657ac-f7aa-5db0-b632-d8bbeb6dfb50";
const AGENT_PUBKEY = TEST_IDENTITIES.charlie.pubkey;

type MockMessageEvent = {
  id: string;
  created_at: number;
  pubkey: string;
};

async function waitForMockLiveSubscription(page: Page, channelName: string) {
  await expect
    .poll(() =>
      page.evaluate(
        (name) =>
          (
            window as Window & {
              __BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?: (input: {
                channelName: string;
              }) => boolean;
            }
          ).__BUZZ_E2E_HAS_MOCK_LIVE_SUBSCRIPTION__?.({
            channelName: name,
          }) ?? false,
        channelName,
      ),
    )
    .toBe(true);
}

async function emitMockMessage(
  page: Page,
  content: string,
  options: {
    parentEventId?: string;
    pubkey: string;
    createdAt: number;
    mentionPubkeys?: string[];
  },
): Promise<MockMessageEvent> {
  const event = await page.evaluate(
    ({ body, parentEventId, pubkey, createdAt, mentionPubkeys }) =>
      (
        window as Window & {
          __BUZZ_E2E_EMIT_MOCK_MESSAGE__?: (input: {
            channelName: string;
            content: string;
            parentEventId?: string;
            pubkey: string;
            createdAt: number;
            mentionPubkeys?: string[];
          }) => MockMessageEvent;
        }
      ).__BUZZ_E2E_EMIT_MOCK_MESSAGE__?.({
        channelName: "general",
        content: body,
        parentEventId,
        pubkey,
        createdAt,
        mentionPubkeys,
      }),
    {
      body: content,
      parentEventId: options.parentEventId,
      pubkey: options.pubkey,
      createdAt: options.createdAt,
      mentionPubkeys: options.mentionPubkeys,
    },
  );
  if (!event) {
    throw new Error("Mock message emitter is unavailable");
  }
  return event;
}

async function seedChannelActivity(
  page: Page,
  {
    extraThreadCount = 0,
    includeAgent = true,
  }: { extraThreadCount?: number; includeAgent?: boolean } = {},
) {
  await page.goto("/");
  await page.getByTestId("channel-general").click();
  await expect(page.getByTestId("chat-title")).toHaveText("general");
  await waitForMockLiveSubscription(page, "general");

  const ownRoot = await emitMockMessage(
    page,
    "How should the handoff work for longer agent tasks?",
    {
      pubkey: SELF_PUBKEY,
      createdAt: Math.floor(Date.now() / 1000) - 20,
    },
  );
  const extraRoots = await Promise.all(
    Array.from({ length: extraThreadCount }, (_, index) =>
      emitMockMessage(page, `Overflow preview thread ${index + 1}`, {
        pubkey: SELF_PUBKEY,
        createdAt: Math.floor(Date.now() / 1000) - 19 + index,
      }),
    ),
  );

  await page.getByTestId("channel-random").click();
  await expect(page.getByTestId("chat-title")).toHaveText("random");

  const unreadAt = Math.floor(Date.now() / 1000) + 60;
  await emitMockMessage(
    page,
    "I tightened the empty state and added the direct thread link.",
    {
      parentEventId: "mock-general-welcome",
      pubkey: TEST_IDENTITIES.alice.pubkey,
      createdAt: unreadAt,
      mentionPubkeys: [SELF_PUBKEY],
    },
  );
  await emitMockMessage(
    page,
    "The updated interaction keeps context visible without opening the channel first.",
    {
      parentEventId: ownRoot.id,
      pubkey: TEST_IDENTITIES.bob.pubkey,
      createdAt: unreadAt + 1,
    },
  );
  await Promise.all(
    extraRoots.map((root, index) =>
      emitMockMessage(
        page,
        `A new reply in overflow preview thread ${index + 1}`,
        {
          parentEventId: root.id,
          pubkey: TEST_IDENTITIES.alice.pubkey,
          createdAt: unreadAt + 2 + index,
        },
      ),
    ),
  );

  if (includeAgent) {
    await page.waitForFunction(
      () =>
        typeof (window as Window & { __BUZZ_E2E_SEED_ACTIVE_TURNS__?: unknown })
          .__BUZZ_E2E_SEED_ACTIVE_TURNS__ === "function",
    );
    await page.evaluate(
      ({ agentPubkey, channelId }) => {
        (
          window as Window & {
            __BUZZ_E2E_SEED_ACTIVE_TURNS__?: (input: {
              agentPubkey: string;
              channelId: string;
              turnId: string;
            }) => void;
          }
        ).__BUZZ_E2E_SEED_ACTIVE_TURNS__?.({
          agentPubkey,
          channelId,
          turnId: "channel-hover-preview",
        });
      },
      { agentPubkey: AGENT_PUBKEY, channelId: CHANNEL_GENERAL },
    );
  }

  await expect(page.getByTestId("channel-unread-dot-general")).toBeVisible();
  if (includeAgent) {
    await expect(page.getByTestId("channel-working-general")).toBeVisible();
  }
}

async function openActivityPopover(page: Page) {
  await page.getByTestId("channel-general").hover();
  const popover = page.getByTestId("channel-activity-popover-general");
  await expect(popover).toBeVisible();
  return popover;
}

test.describe("channel activity hover preview", () => {
  test.beforeEach(async ({ page }) => {
    await installMockBridge(page, {
      managedAgents: [
        {
          pubkey: AGENT_PUBKEY,
          name: "Charlie",
          status: "running",
          channelNames: ["general"],
        },
      ],
    });
  });

  test("shows unread channel activity and working agents, then opens the selected thread", async ({
    page,
  }) => {
    await seedChannelActivity(page, { extraThreadCount: 5 });
    const popover = await openActivityPopover(page);

    const heading = popover.getByRole("heading", {
      name: "Channel activity",
    });
    await expect(heading).toBeVisible();
    await expect(heading).toHaveCSS("backdrop-filter", /blur/);
    const activityScroll = popover.getByTestId("channel-activity-scroll");
    await expect(activityScroll).toHaveCSS("overflow-y", "auto");
    await expect(activityScroll.getByRole("heading")).toHaveCount(0);
    await expect
      .poll(() =>
        activityScroll.evaluate(
          (element) => element.scrollHeight > element.clientHeight,
        ),
      )
      .toBe(true);
    await waitForAnimations(page);
    const headingY = (await heading.boundingBox())?.y;
    const scrollY = (await activityScroll.boundingBox())?.y;
    const headingBottom = await heading.evaluate(
      (element) => element.getBoundingClientRect().bottom,
    );
    expect(scrollY).toBeGreaterThanOrEqual(headingBottom - 1);
    await activityScroll.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    await expect
      .poll(() => activityScroll.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);
    const scrolledHeadingY = (await heading.boundingBox())?.y;
    expect(headingY).toBeDefined();
    expect(scrolledHeadingY).toBeDefined();
    expect(Math.abs((scrolledHeadingY ?? 0) - (headingY ?? 0))).toBeLessThan(
      0.5,
    );
    await activityScroll.evaluate((element) => {
      element.scrollTop = 0;
    });
    await expect
      .poll(() =>
        activityScroll.evaluate(
          (element) =>
            getComputedStyle(element, "::-webkit-scrollbar-thumb")
              .backgroundColor,
        ),
      )
      .toMatch(/0\.15\)/);
    await expect(popover.getByTestId(/^channel-activity-item-/)).toHaveCount(7);
    await expect(popover).toContainText(
      "I tightened the empty state and added the direct thread link.",
    );
    await expect(popover.getByRole("heading")).toHaveCount(1);
    await expect(
      popover.getByTestId(`channel-activity-agent-${AGENT_PUBKEY}`),
    ).toContainText("Charlie");
    await expect(
      popover.getByTestId(`channel-activity-agent-${AGENT_PUBKEY}`),
    ).toContainText("Working");
    const orderedRows = popover.locator(
      '[data-testid^="channel-activity-agent-"], [data-testid^="channel-activity-item-"]',
    );
    await expect(orderedRows.first()).toHaveAttribute(
      "data-testid",
      `channel-activity-agent-${AGENT_PUBKEY}`,
    );

    await waitForAnimations(page);
    await page.screenshot({
      clip: { x: 0, y: 80, width: 720, height: 640 },
      path: "test-results/channel-activity-hover/channel-activity-popover.png",
    });

    const targetRow = popover
      .getByTestId(/^channel-activity-item-/)
      .filter({ hasText: "direct thread link" });
    await targetRow.getByRole("button", { name: /Open thread/ }).click();

    await expect(page.getByTestId("chat-title")).toHaveText("general");
    const threadPanel = page.getByTestId("message-thread-panel");
    await expect(threadPanel).toBeVisible();
    await expect(threadPanel).toContainText("direct thread link");
  });

  test("removes the dot and preview after the final activity is read", async ({
    page,
  }) => {
    await seedChannelActivity(page, { includeAgent: false });
    const popover = await openActivityPopover(page);
    await expect(popover.getByTestId(/^channel-activity-item-/)).toHaveCount(2);

    for (let remaining = 1; remaining >= 0; remaining -= 1) {
      const row = popover.getByTestId(/^channel-activity-item-/).first();
      await row.hover();
      await row.getByRole("button", { name: "Mark as read" }).click();
      await expect(popover.getByTestId(/^channel-activity-item-/)).toHaveCount(
        remaining,
      );
    }

    await expect(page.getByTestId("channel-unread-dot-general")).toHaveCount(0);
    await expect(
      page.getByTestId("channel-activity-popover-general"),
    ).toHaveCount(0);
  });

  test("supports row actions and opens an agent's scoped activity", async ({
    page,
  }) => {
    await seedChannelActivity(page);
    let popover = await openActivityPopover(page);
    const initialRows = popover.getByTestId(/^channel-activity-item-/);
    await expect(initialRows).toHaveCount(2);

    const firstRow = initialRows.first();
    await firstRow.hover();
    await firstRow.getByRole("button", { name: "Mark as read" }).click();
    await expect(popover.getByTestId(/^channel-activity-item-/)).toHaveCount(1);

    const remainingRow = popover.getByTestId(/^channel-activity-item-/).first();
    await remainingRow.hover();
    await remainingRow.getByRole("button", { name: "Remind me later" }).click();
    await expect(
      page.getByRole("dialog").getByText("Remind me later"),
    ).toBeVisible();
    await page.getByRole("button", { name: "Cancel" }).click();

    popover = await openActivityPopover(page);
    await popover.getByTestId(`channel-activity-agent-${AGENT_PUBKEY}`).click();

    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await expect(page.getByTestId("agent-session-agent-name")).toContainText(
      "Charlie",
    );
    await expect(page.getByTestId("agent-session-scope-label")).toContainText(
      "#general",
    );
  });

  test("keeps multiple Inbox threads visible after opening their channel", async ({
    page,
  }) => {
    const inboxItemIds = [
      "older-inbox-thread-for-hover",
      "second-inbox-thread-for-hover",
    ];
    await page.goto("/");
    await expect(page.getByTestId("home-inbox-list")).toBeVisible();
    await page.waitForFunction(
      () =>
        typeof (
          window as Window & { __BUZZ_E2E_PUSH_MOCK_FEED_ITEM__?: unknown }
        ).__BUZZ_E2E_PUSH_MOCK_FEED_ITEM__ === "function",
    );
    await page.evaluate(
      ({ channelId, currentPubkey, itemIds, senderPubkey }) => {
        const pushFeedItem = (
          window as Window & {
            __BUZZ_E2E_PUSH_MOCK_FEED_ITEM__?: (item: {
              category: "mention";
              channel_id: string;
              channel_name: string;
              channel_type: "stream";
              content: string;
              created_at: number;
              id: string;
              kind: number;
              pubkey: string;
              tags: string[][];
            }) => void;
          }
        ).__BUZZ_E2E_PUSH_MOCK_FEED_ITEM__;
        if (!pushFeedItem) {
          throw new Error("Mock feed injection helper is unavailable");
        }
        const createdAt = Math.floor(Date.now() / 1_000) - 300;
        for (const item of [
          {
            content: "Older Inbox thread reopened for hover testing.",
            id: itemIds[0],
            tags: [
              ["h", channelId],
              ["e", "older-inbox-thread-root", "", "root"],
              ["e", "older-inbox-thread-parent", "", "reply"],
              ["p", currentPubkey],
            ],
          },
          {
            content: "A second Inbox thread reopened for hover testing.",
            id: itemIds[1],
            tags: [
              ["h", channelId],
              ["e", "second-inbox-thread-root", "", "root"],
              ["e", "second-inbox-thread-parent", "", "reply"],
              ["p", currentPubkey],
            ],
          },
        ]) {
          pushFeedItem({
            category: "mention",
            channel_id: channelId,
            channel_name: "general",
            channel_type: "stream",
            content: item.content,
            created_at: createdAt,
            id: item.id,
            kind: 9,
            pubkey: senderPubkey,
            tags: item.tags,
          });
        }
      },
      {
        channelId: CHANNEL_GENERAL,
        currentPubkey: TEST_IDENTITIES.tyler.pubkey,
        itemIds: inboxItemIds,
        senderPubkey: TEST_IDENTITIES.alice.pubkey,
      },
    );

    for (const itemId of inboxItemIds) {
      const inboxRow = page.getByTestId(`home-inbox-item-${itemId}`);
      await expect(inboxRow).toBeVisible();
      await inboxRow.hover();
      await inboxRow.getByRole("button", { name: "Mark as read" }).click();
      await inboxRow.hover();
      await inboxRow.getByRole("button", { name: "Mark unread" }).click();
    }

    await expect(page.getByTestId("channel-general")).toHaveCSS(
      "font-weight",
      "600",
    );
    await expect(page.getByTestId("channel-unread-dot-general")).toBeVisible();
    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await expect(page.getByTestId("channel-unread-dot-general")).toBeVisible();
    await page.mouse.move(900, 680);
    const popover = await openActivityPopover(page);
    await expect(popover.getByTestId(/^channel-activity-item-/)).toHaveCount(2);
    await expect(popover).toContainText("Older Inbox thread reopened");
    await expect(popover).toContainText("A second Inbox thread reopened");
    await expect(popover.getByText("Thread", { exact: true })).toHaveCount(2);
  });

  test("surfaces future replies after the user reacts to a thread root", async ({
    page,
  }) => {
    await page.goto("/");
    await page.getByTestId("channel-general").click();
    await expect(page.getByTestId("chat-title")).toHaveText("general");
    await waitForMockLiveSubscription(page, "general");

    const root = await emitMockMessage(
      page,
      "Reacting here means I care about future replies.",
      {
        pubkey: TEST_IDENTITIES.alice.pubkey,
        createdAt: Math.floor(Date.now() / 1_000) - 20,
      },
    );
    const rootRow = page
      .getByTestId("message-row")
      .filter({ hasText: "Reacting here means I care" });
    await expect(rootRow).toBeVisible();
    await rootRow.hover();
    const actionBar = page.getByTestId(`message-action-bar-${root.id}`);
    await expect(actionBar).toBeVisible();
    await actionBar
      .getByRole("button", { name: /^React with / })
      .first()
      .click();
    await expect(
      rootRow.getByRole("button", { name: /^Toggle .* reaction$/ }),
    ).toBeVisible();

    await page.getByTestId("channel-random").click();
    await emitMockMessage(
      page,
      "This follow-up appears because the root was reacted to.",
      {
        parentEventId: root.id,
        pubkey: TEST_IDENTITIES.bob.pubkey,
        createdAt: Math.floor(Date.now() / 1_000) + 60,
      },
    );

    await expect(page.getByTestId("channel-unread-dot-general")).toBeVisible();
    const popover = await openActivityPopover(page);
    await expect(popover).toContainText(
      "This follow-up appears because the root was reacted to.",
    );
  });
});
