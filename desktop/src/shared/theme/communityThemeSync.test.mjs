import assert from "node:assert/strict";
import test, { mock } from "node:test";
import { relayClient } from "@/shared/api/relayClient";
import {
  CommunityThemeSyncManager,
  shouldSeedCommunityTheme,
} from "./communityThemeSync.ts";

const preference = {
  version: 1,
  theme: "houston",
  accent: "#3b82f6",
  followSystem: false,
};

function installFakeTimer() {
  globalThis.window ??= {};
  let callback = null;
  const originalSet = window.setTimeout;
  const originalClear = window.clearTimeout;
  window.setTimeout = (fn) => {
    callback = fn;
    return 1;
  };
  window.clearTimeout = () => {
    callback = null;
  };
  return {
    fire: () => {
      const fn = callback;
      callback = null;
      fn?.();
    },
    pending: () => callback !== null,
    restore: () => {
      window.setTimeout = originalSet;
      window.clearTimeout = originalClear;
    },
  };
}

test("destroy cancels a debounced community write before relay teardown", () => {
  const timer = installFakeTimer();
  const publishes = [];
  mock.method(relayClient, "publishEvent", (...args) => {
    publishes.push(args);
    return Promise.resolve();
  });
  try {
    const manager = new CommunityThemeSyncManager("alice");
    manager.publish(preference);
    assert.equal(timer.pending(), true);
    manager.destroy();
    assert.equal(timer.pending(), false);
    timer.fire();
    assert.equal(publishes.length, 0);
  } finally {
    timer.restore();
    mock.reset();
  }
});

test("destroy is safe without a pending community write", () => {
  const manager = new CommunityThemeSyncManager("alice");
  assert.doesNotThrow(() => manager.destroy());
  assert.equal(manager.getPending(), null);
});

function relayEvent(overrides = {}) {
  return {
    id: "event-id",
    pubkey: "alice",
    kind: 30078,
    content: "not-decryptable",
    created_at: 123,
    tags: [["d", "community-theme"]],
    ...overrides,
  };
}

test("fetch distinguishes absent remote state from unreadable existing state", async () => {
  mock.method(relayClient, "fetchEvents", () => Promise.resolve([]));
  try {
    const manager = new CommunityThemeSyncManager("alice");
    assert.deepEqual(await manager.fetchRemote(), { status: "absent" });
  } finally {
    mock.reset();
  }

  mock.method(relayClient, "fetchEvents", () =>
    Promise.resolve([relayEvent()]),
  );
  try {
    const manager = new CommunityThemeSyncManager("alice");
    assert.deepEqual(await manager.fetchRemote(), { status: "invalid" });
  } finally {
    mock.reset();
  }
});

test("fetch reports relay failures as unavailable rather than absent", async () => {
  mock.method(relayClient, "fetchEvents", () =>
    Promise.reject(new Error("offline")),
  );
  try {
    const manager = new CommunityThemeSyncManager("alice");
    assert.deepEqual(await manager.fetchRemote(), { status: "unavailable" });
  } finally {
    mock.reset();
  }
});

test("only confirmed absence permits seeding relay state", () => {
  assert.equal(shouldSeedCommunityTheme({ status: "absent" }), true);
  assert.equal(shouldSeedCommunityTheme({ status: "invalid" }), false);
  assert.equal(shouldSeedCommunityTheme({ status: "unavailable" }), false);
});
