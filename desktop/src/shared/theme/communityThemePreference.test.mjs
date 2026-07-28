import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_COMMUNITY_THEME,
  cacheAndApplyCommunityTheme,
  communityThemeStorageKey,
  parseCommunityThemePreference,
  readCommunityThemePreference,
  writeCommunityThemePreference,
} from "./communityThemePreference.ts";

function localStorageStub() {
  const data = new Map();
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => data.set(key, String(value)),
  };
}

test("parses only the versioned stable appearance contract", () => {
  const valid = {
    version: 1,
    theme: "houston",
    accent: "#a855f7",
    followSystem: false,
  };
  assert.deepEqual(parseCommunityThemePreference(valid), valid);
  assert.equal(parseCommunityThemePreference({ ...valid, version: 2 }), null);
  assert.equal(
    parseCommunityThemePreference({ ...valid, theme: "future-theme" }),
    null,
  );
  assert.equal(
    parseCommunityThemePreference({ ...valid, accent: "url(image)" }),
    null,
  );
  assert.equal(
    parseCommunityThemePreference({ ...valid, followSystem: "false" }),
    null,
  );
});

test("local preferences are isolated by pubkey and normalized relay", () => {
  globalThis.window = { localStorage: localStorageStub() };
  const aliceA = {
    ...DEFAULT_COMMUNITY_THEME,
    theme: "houston",
    followSystem: false,
  };
  const aliceB = { ...DEFAULT_COMMUNITY_THEME, theme: "catppuccin-latte" };
  const bobA = { ...DEFAULT_COMMUNITY_THEME, accent: "#ef4444" };
  assert.equal(
    writeCommunityThemePreference("alice", "WSS://A.EXAMPLE/", aliceA),
    true,
  );
  assert.equal(
    writeCommunityThemePreference("alice", "wss://b.example", aliceB),
    true,
  );
  assert.equal(
    writeCommunityThemePreference("bob", "wss://a.example", bobA),
    true,
  );
  assert.deepEqual(
    readCommunityThemePreference("alice", "wss://a.example"),
    aliceA,
  );
  assert.deepEqual(
    readCommunityThemePreference("alice", "wss://b.example/"),
    aliceB,
  );
  assert.deepEqual(
    readCommunityThemePreference("bob", "wss://a.example"),
    bobA,
  );
  assert.notEqual(
    communityThemeStorageKey("alice", "wss://a.example"),
    communityThemeStorageKey("alice", "wss://b.example"),
  );
});

test("malformed local data returns null so switching can apply the safe default", () => {
  globalThis.window = { localStorage: localStorageStub() };
  const key = communityThemeStorageKey("alice", "wss://broken.example");
  window.localStorage.setItem(
    key,
    JSON.stringify({ version: 1, theme: "missing" }),
  );
  assert.equal(
    readCommunityThemePreference("alice", "wss://broken.example"),
    null,
  );
  window.localStorage.setItem(key, "{");
  assert.equal(
    readCommunityThemePreference("alice", "wss://broken.example"),
    null,
  );
});

test("remote preference still applies when its local cache write fails", () => {
  globalThis.window = {
    localStorage: {
      getItem: () => null,
      setItem: () => {
        throw new Error("quota exceeded");
      },
    },
  };
  let applied = null;
  cacheAndApplyCommunityTheme(
    "alice",
    "wss://a.example",
    DEFAULT_COMMUNITY_THEME,
    (preference) => {
      applied = preference;
    },
  );
  assert.deepEqual(applied, DEFAULT_COMMUNITY_THEME);
});
