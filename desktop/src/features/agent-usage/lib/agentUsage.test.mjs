import assert from "node:assert/strict";
import test from "node:test";

import {
  bigintRatio,
  buildCustomDayBoundaries,
  buildLocalDayBoundaries,
  buildRangeBoundaries,
  countRangeDays,
  describeRange,
  deriveDisplayTotal,
  deriveUsageIngressTrailing,
  formatCoverageDate,
  formatEstimatedCostUsd,
  formatLocalDate,
  formatTokenCountCompact,
  formatTokenCountExact,
  isPartialField,
  isUnknownField,
  MAX_RANGE_DAYS,
  msUntilNextLocalMidnight,
  parseLocalDate,
  parseTokenCount,
  sortAgentsByDisplayTotal,
  sortModelsByDisplayTotal,
  sumKnownBucketTotals,
  validateCustomRange,
} from "./agentUsage.ts";

// ── Fixtures ─────────────────────────────────────────────────────────────────

function usageField(overrides = {}) {
  return { value: null, incomplete: false, ...overrides };
}

function reportedUsage(overrides = {}) {
  return {
    inputTokens: usageField(),
    outputTokens: usageField(),
    totalTokens: usageField(),
    estimatedCostUsd: usageField(),
    ...overrides,
  };
}

function agentUsage(pubkey, totalTokensValue, overrides = {}) {
  return {
    agentPubkey: pubkey,
    usage: reportedUsage({
      totalTokens: usageField({ value: totalTokensValue }),
    }),
    buckets: [],
    models: [],
    reportCount: 0,
    hasUnknownUsage: false,
    ...overrides,
  };
}

function modelUsage(model, totalTokensValue, overrides = {}) {
  return {
    harness: null,
    model,
    usage: reportedUsage({
      totalTokens: usageField({ value: totalTokensValue }),
    }),
    reportCount: 0,
    hasUnknownUsage: false,
    ...overrides,
  };
}

// ── buildLocalDayBoundaries ──────────────────────────────────────────────────

test("buildLocalDayBoundaries returns 8 boundaries for a 7-day window", () => {
  const now = new Date(2026, 5, 15, 14, 30, 0); // June 15, 2026, 14:30 local
  const boundaries = buildLocalDayBoundaries(7, now);
  assert.equal(boundaries.length, 8);
});

test("buildLocalDayBoundaries returns 31 boundaries for a 30-day window", () => {
  const now = new Date(2026, 5, 15, 14, 30, 0);
  const boundaries = buildLocalDayBoundaries(30, now);
  assert.equal(boundaries.length, 31);
});

test("buildLocalDayBoundaries produces strictly increasing boundaries ending at tomorrow's local midnight", () => {
  const now = new Date(2026, 5, 15, 14, 30, 0);
  const boundaries = buildLocalDayBoundaries(7, now);
  for (let i = 1; i < boundaries.length; i++) {
    assert.ok(
      boundaries[i] > boundaries[i - 1],
      `boundary ${i} must exceed boundary ${i - 1}`,
    );
  }
  const tomorrowMidnight = new Date(2026, 5, 16, 0, 0, 0, 0);
  assert.equal(
    boundaries.at(-1),
    Math.floor(tomorrowMidnight.getTime() / 1000),
  );
});

test("buildLocalDayBoundaries is independent of time-of-day within the reference day", () => {
  const morning = buildLocalDayBoundaries(7, new Date(2026, 5, 15, 0, 0, 1));
  const night = buildLocalDayBoundaries(7, new Date(2026, 5, 15, 23, 59, 59));
  assert.deepEqual(morning, night);
});

// ── buildLocalDayBoundaries / msUntilNextLocalMidnight explicit-TZ coverage ──
//
// `process.env.TZ` is read by the JS engine on every `Date` field access
// (not just pinned at process start), so each test below mutates it
// directly and restores the original value in a `finally` — no subprocess
// needed, but evidence (`Date#toString()`) is asserted so a future runtime
// that DOES pin `TZ` at startup fails loudly instead of silently passing
// against the wrong offset.

function withTz(tz, fn) {
  const original = process.env.TZ;
  process.env.TZ = tz;
  try {
    return fn();
  } finally {
    if (original === undefined) delete process.env.TZ;
    else process.env.TZ = original;
  }
}

function assertStrictlyIncreasing(boundaries) {
  for (let i = 1; i < boundaries.length; i++) {
    assert.ok(
      boundaries[i] > boundaries[i - 1],
      `boundary ${i} (${boundaries[i]}) must exceed boundary ${i - 1} (${boundaries[i - 1]})`,
    );
  }
}

test("buildLocalDayBoundaries stays strictly increasing across an ordinary US spring-forward DST transition", () => {
  withTz("America/New_York", () => {
    const now = new Date(2024, 2, 12, 10, 0, 0); // Mar 12, after Mar 10 spring-forward
    assert.equal(
      now.toString().includes("Daylight"),
      true,
      "sanity: DST active",
    );
    const boundaries = buildLocalDayBoundaries(7, now);
    assert.equal(boundaries.length, 8);
    assertStrictlyIncreasing(boundaries);
  });
});

test("buildLocalDayBoundaries stays strictly increasing across an ordinary US fall-back DST transition", () => {
  withTz("America/New_York", () => {
    const now = new Date(2024, 10, 6, 10, 0, 0); // Nov 6, after Nov 3 fall-back
    assert.equal(
      now.toString().includes("Standard"),
      true,
      "sanity: standard time active",
    );
    const boundaries = buildLocalDayBoundaries(7, now);
    assert.equal(boundaries.length, 8);
    assertStrictlyIncreasing(boundaries);
  });
});

test("buildLocalDayBoundaries stays strictly increasing across Lord Howe Island's 30-minute DST shift", () => {
  withTz("Australia/Lord_Howe", () => {
    const now = new Date(2024, 9, 8, 10, 0, 0); // Oct 8, after Oct 6 spring-forward (+30min)
    const boundaries = buildLocalDayBoundaries(7, now);
    assert.equal(boundaries.length, 8);
    assertStrictlyIncreasing(boundaries);
    // The transition day is a 23.5h day, not the ordinary 24h.
    const deltas = [];
    for (let i = 1; i < boundaries.length; i++) {
      deltas.push(boundaries[i] - boundaries[i - 1]);
    }
    assert.ok(
      deltas.some((d) => d === 23.5 * 3600),
      `expected a 23.5h transition delta, got ${JSON.stringify(deltas)}`,
    );
  });
});

test("buildLocalDayBoundaries stays strictly increasing across Pacific/Apia's skipped 2011-12-30 civil date", () => {
  withTz("Pacific/Apia", () => {
    // Samoa skipped 2011-12-30 entirely moving across the International
    // Date Line; Dec 29 was immediately followed by Dec 31. The UTC
    // offset itself jumped by exactly 24h (UTC-11 -> UTC+13) at that
    // instant, so real elapsed time across the 2-civil-day jump is only
    // 24h, not 48h — but the boundary construction must not collapse the
    // two distinct local midnights (Dec 29, Dec 31) into one duplicate.
    const now = new Date(2011, 11, 31, 12, 0, 0);
    const boundaries = buildLocalDayBoundaries(7, now);
    assert.equal(boundaries.length, 8);
    assertStrictlyIncreasing(boundaries);
    const dec29 = Math.floor(new Date(2011, 11, 29, 0, 0, 0).getTime() / 1000);
    const dec31 = Math.floor(new Date(2011, 11, 31, 0, 0, 0).getTime() / 1000);
    assert.ok(
      boundaries.includes(dec29),
      "expected Dec 29 local midnight as a boundary",
    );
    assert.ok(
      boundaries.includes(dec31),
      "expected Dec 31 local midnight as a boundary",
    );
  });
});

test("buildLocalDayBoundaries produces days+1 boundaries even when a civil date is skipped", () => {
  withTz("Pacific/Apia", () => {
    const boundaries7 = buildLocalDayBoundaries(
      7,
      new Date(2011, 11, 31, 12, 0, 0),
    );
    assert.equal(boundaries7.length, 8);
    const boundaries30 = buildLocalDayBoundaries(
      30,
      new Date(2011, 11, 31, 12, 0, 0),
    );
    assert.equal(boundaries30.length, 31);
    assertStrictlyIncreasing(boundaries30);
  });
});

// ── msUntilNextLocalMidnight ─────────────────────────────────────────────────

test("msUntilNextLocalMidnight returns the exact gap to the next local midnight", () => {
  const now = new Date(2026, 5, 15, 23, 0, 0, 0);
  const ms = msUntilNextLocalMidnight(now);
  assert.equal(ms, 60 * 60 * 1000);
});

test("msUntilNextLocalMidnight is always positive, even called exactly at midnight", () => {
  const now = new Date(2026, 5, 15, 0, 0, 0, 0);
  const ms = msUntilNextLocalMidnight(now);
  assert.equal(ms, 24 * 60 * 60 * 1000);
});

test("msUntilNextLocalMidnight is positive and lands on a real local midnight across DST/date-line TZs", () => {
  const cases = [
    ["America/New_York", new Date(2024, 2, 9, 23, 30, 0)], // eve of spring-forward
    ["Australia/Lord_Howe", new Date(2024, 9, 5, 23, 45, 0)], // eve of 30-min shift
    ["Pacific/Apia", new Date(2011, 11, 29, 23, 0, 0)], // eve of the skipped date
  ];
  for (const [tz, now] of cases) {
    withTz(tz, () => {
      const ms = msUntilNextLocalMidnight(now);
      assert.ok(ms > 0, `${tz}: expected positive ms, got ${ms}`);
      const landed = new Date(now.getTime() + ms);
      assert.equal(
        landed.getHours(),
        0,
        `${tz}: expected to land on local midnight, got ${landed.toString()}`,
      );
      assert.equal(landed.getMinutes(), 0, `${tz}: expected :00 minutes`);
    });
  }
});

// ── parseTokenCount ──────────────────────────────────────────────────────────

test("parseTokenCount parses a plain decimal string to bigint", () => {
  assert.equal(parseTokenCount("12345"), 12345n);
});

test("parseTokenCount preserves u64::MAX precision beyond Number.MAX_SAFE_INTEGER", () => {
  assert.equal(parseTokenCount("18446744073709551615"), 18446744073709551615n);
});

test("parseTokenCount returns null for null input", () => {
  assert.equal(parseTokenCount(null), null);
});

test("parseTokenCount fails closed on malformed wire data instead of throwing", () => {
  for (const malformed of ["", "-1", "1.5", "abc", "1e10", " 1", "1 "]) {
    assert.equal(
      parseTokenCount(malformed),
      null,
      `expected null for ${JSON.stringify(malformed)}`,
    );
  }
});

test("parseTokenCount accepts zero", () => {
  assert.equal(parseTokenCount("0"), 0n);
});

// ── formatTokenCountCompact / formatTokenCountExact ─────────────────────────

test("formatTokenCountCompact abbreviates thousands/millions/billions", () => {
  assert.equal(formatTokenCountCompact(999n), "999");
  assert.equal(formatTokenCountCompact(1_234n), "1.2K");
  assert.equal(formatTokenCountCompact(1_000_000n), "1M");
  assert.equal(formatTokenCountCompact(1_500_000_000n), "1.5B");
});

test("formatTokenCountCompact handles negative magnitudes symmetrically", () => {
  assert.equal(formatTokenCountCompact(-1_234n), "-1.2K");
});

test("formatTokenCountExact renders full grouped digits, never abbreviated", () => {
  assert.equal(formatTokenCountExact(1_234_567n), "1,234,567");
  assert.equal(formatTokenCountExact(0n), "0");
});

// ── formatEstimatedCostUsd ───────────────────────────────────────────────────

test("formatEstimatedCostUsd renders two-decimal USD currency", () => {
  assert.equal(formatEstimatedCostUsd(1.5), "$1.50");
  assert.equal(formatEstimatedCostUsd(0), "$0.00");
});

// ── formatCoverageDate ───────────────────────────────────────────────────────

test("formatCoverageDate renders unknown for a missing timestamp", () => {
  assert.equal(formatCoverageDate(null), "unknown");
});

test("formatCoverageDate renders a timestamp as its local month and day without a year", () => {
  const unixSeconds = 1_737_849_600; // 2025-01-26T00:00:00Z
  const localDate = new Date(unixSeconds * 1000);
  const formatted = formatCoverageDate(unixSeconds);

  assert.match(formatted, new RegExp(`\\b${localDate.getDate()}\\b`));
  assert.doesNotMatch(formatted, new RegExp(`${localDate.getFullYear()}`));
});

// ── bigintRatio ──────────────────────────────────────────────────────────────

test("bigintRatio computes a bounded ratio without losing bigint precision on large magnitudes", () => {
  const whole = 18_446_744_073_709_551_614n; // largest even value near u64::MAX
  assert.equal(bigintRatio(whole / 2n, whole), 0.5);
});

test("bigintRatio returns 0 for a zero or negative whole (divide-by-zero guard)", () => {
  assert.equal(bigintRatio(5n, 0n), 0);
  assert.equal(bigintRatio(5n, -10n), 0);
});

test("bigintRatio clamps part to [0, whole]", () => {
  assert.equal(bigintRatio(-5n, 100n), 0);
  assert.equal(bigintRatio(200n, 100n), 1);
});

// ── deriveDisplayTotal ────────────────────────────────────────────────────────

test("deriveDisplayTotal returns exact kind when totalTokens is present", () => {
  const usage = reportedUsage({
    inputTokens: usageField({ value: "800" }),
    outputTokens: usageField({ value: "200" }),
    totalTokens: usageField({ value: "1100" }),
  });
  const dt = deriveDisplayTotal(usage);
  assert.equal(dt.kind, "exact");
  assert.equal(dt.value, 1100n);
  assert.equal(dt.partial, false);
});

test("deriveDisplayTotal carries partial=true for an exact total flagged incomplete", () => {
  const usage = reportedUsage({
    totalTokens: usageField({ value: "900", incomplete: true }),
  });
  const dt = deriveDisplayTotal(usage);
  assert.equal(dt.kind, "exact");
  assert.equal(dt.value, 900n);
  assert.equal(dt.partial, true);
});

test("deriveDisplayTotal returns approximate kind when totalTokens is null but i/o is known", () => {
  const usage = reportedUsage({
    inputTokens: usageField({ value: "800" }),
    outputTokens: usageField({ value: "200" }),
  });
  const dt = deriveDisplayTotal(usage);
  assert.equal(dt.kind, "approximate");
  assert.equal(dt.value, 1000n);
  assert.equal(dt.partial, false);
});

test("deriveDisplayTotal approximate partial=true when either i/o field is incomplete", () => {
  const usage = reportedUsage({
    inputTokens: usageField({ value: "800", incomplete: true }),
    outputTokens: usageField({ value: "200" }),
  });
  const dt = deriveDisplayTotal(usage);
  assert.equal(dt.kind, "approximate");
  assert.equal(dt.partial, true);
});

test("deriveDisplayTotal returns unknown when only input is known and output is null (fail-closed)", () => {
  const usage = reportedUsage({
    inputTokens: usageField({ value: "500" }),
    // outputTokens: null (default)
  });
  const dt = deriveDisplayTotal(usage);
  assert.equal(dt.kind, "unknown");
  assert.equal(dt.value, null);
});

test("deriveDisplayTotal returns unknown when only output is known and input is null (fail-closed)", () => {
  const usage = reportedUsage({
    outputTokens: usageField({ value: "300" }),
    // inputTokens: null (default)
  });
  const dt = deriveDisplayTotal(usage);
  assert.equal(dt.kind, "unknown");
  assert.equal(dt.value, null);
});

test("deriveDisplayTotal returns unknown kind when all fields are null", () => {
  const usage = reportedUsage();
  const dt = deriveDisplayTotal(usage);
  assert.equal(dt.kind, "unknown");
  assert.equal(dt.value, null);
  assert.equal(dt.partial, false);
});

// ── sortAgentsByDisplayTotal / sortModelsByDisplayTotal ─────────────────────

test("sortAgentsByDisplayTotal ranks known exact totals descending", () => {
  const agents = [
    agentUsage("a1", "100"),
    agentUsage("a2", "300"),
    agentUsage("a3", "200"),
  ];
  const sorted = sortAgentsByDisplayTotal(agents);
  assert.deepEqual(
    sorted.map((a) => a.agentPubkey),
    ["a2", "a3", "a1"],
  );
});

test("sortAgentsByDisplayTotal ranks exact totals above approximate totals", () => {
  const exactAgent = agentUsage("exact", "50");
  const approxAgent = agentUsage("approx", null, {
    usage: reportedUsage({
      inputTokens: usageField({ value: "9000" }),
      outputTokens: usageField({ value: "9000" }),
    }),
  });
  const sorted = sortAgentsByDisplayTotal([approxAgent, exactAgent]);
  // exact(50) < approximate(18000) numerically, but exact tier wins
  assert.equal(sorted[0].agentPubkey, "exact");
  assert.equal(sorted[1].agentPubkey, "approx");
});

test("sortAgentsByDisplayTotal ranks approximate totals above unknown totals", () => {
  const approxAgent = agentUsage("approx", null, {
    usage: reportedUsage({
      inputTokens: usageField({ value: "100" }),
      outputTokens: usageField({ value: "50" }),
    }),
  });
  const unknownAgent = agentUsage("unknown", null);
  const sorted = sortAgentsByDisplayTotal([unknownAgent, approxAgent]);
  assert.equal(sorted[0].agentPubkey, "approx");
  assert.equal(sorted[1].agentPubkey, "unknown");
});

test("sortAgentsByDisplayTotal handles mixed exact/approximate/unknown population in tier order", () => {
  const agents = [
    agentUsage("u1", null), // unknown
    agentUsage("e1", "100"), // exact
    agentUsage("a1", null, {
      // approximate
      usage: reportedUsage({
        inputTokens: usageField({ value: "400" }),
        outputTokens: usageField({ value: "100" }),
      }),
    }),
    agentUsage("u2", null), // unknown
    agentUsage("e2", "300"), // exact
    agentUsage("a2", null, {
      // approximate
      usage: reportedUsage({
        inputTokens: usageField({ value: "150" }),
        outputTokens: usageField({ value: "50" }),
      }),
    }),
  ];
  const sorted = sortAgentsByDisplayTotal(agents);
  // Tier order: exact first (e2=300 > e1=100), then approx (a1=500 > a2=200), then unknown (u1 < u2 by pubkey)
  assert.deepEqual(
    sorted.map((a) => a.agentPubkey),
    ["e2", "e1", "a1", "a2", "u1", "u2"],
  );
});

test("sortAgentsByDisplayTotal lists unknown-total agents after all other agents, tiebroken by pubkey", () => {
  const agents = [
    agentUsage("unknown-b", null),
    agentUsage("known", "50"),
    agentUsage("unknown-a", null),
  ];
  const sorted = sortAgentsByDisplayTotal(agents);
  assert.equal(sorted[0].agentPubkey, "known");
  assert.deepEqual(
    sorted.slice(1).map((a) => a.agentPubkey),
    ["unknown-a", "unknown-b"],
  );
});

test("sortAgentsByDisplayTotal tiebreaks equal exact totals by pubkey", () => {
  const agents = [agentUsage("b", "100"), agentUsage("a", "100")];
  const sorted = sortAgentsByDisplayTotal(agents);
  assert.deepEqual(
    sorted.map((a) => a.agentPubkey),
    ["a", "b"],
  );
});

test("sortModelsByDisplayTotal sorts null model ('Unknown model') last among ties", () => {
  const models = [
    modelUsage(null, "100"),
    modelUsage("gpt-4", "100"),
    modelUsage("claude", "100"),
  ];
  const sorted = sortModelsByDisplayTotal(models);
  assert.deepEqual(
    sorted.map((m) => m.model),
    ["claude", "gpt-4", null],
  );
});

test("sortModelsByDisplayTotal tiebreaks harness before model when totals are equal", () => {
  const models = [
    modelUsage("m", "100", { harness: "z-harness" }),
    modelUsage("m", "100", { harness: "a-harness" }),
    modelUsage("m", "100", { harness: null }),
  ];
  const sorted = sortModelsByDisplayTotal(models);
  assert.deepEqual(
    sorted.map((m) => m.harness),
    ["a-harness", "z-harness", null],
  );
});

test("sortModelsByDisplayTotal same model two harnesses produces two rows in harness order", () => {
  // Same model via two harnesses should be distinct rows; harness-ascending tiebreak.
  const models = [
    modelUsage("claude-sonnet", "500", { harness: "goose" }),
    modelUsage("claude-sonnet", "500", { harness: "claude-code" }),
  ];
  const sorted = sortModelsByDisplayTotal(models);
  assert.deepEqual(
    sorted.map((m) => m.harness),
    ["claude-code", "goose"],
  );
});

test("sortModelsByDisplayTotal ranks exact tier above approximate tier regardless of value", () => {
  const exactModel = modelUsage("small-model", "10");
  const approxModel = modelUsage("big-approx", null, {
    usage: reportedUsage({
      inputTokens: usageField({ value: "9999" }),
      outputTokens: usageField({ value: "9999" }),
    }),
  });
  const sorted = sortModelsByDisplayTotal([approxModel, exactModel]);
  assert.equal(sorted[0].model, "small-model"); // exact tier wins
  assert.equal(sorted[1].model, "big-approx");
});

// ── isPartialField / isUnknownField ──────────────────────────────────────────

test("isPartialField is true only for a known value flagged incomplete", () => {
  assert.equal(
    isPartialField(usageField({ value: "10", incomplete: true })),
    true,
  );
  assert.equal(
    isPartialField(usageField({ value: "10", incomplete: false })),
    false,
  );
  assert.equal(
    isPartialField(usageField({ value: null, incomplete: true })),
    false,
  );
});

test("isUnknownField is true only when there is no known value at all", () => {
  assert.equal(isUnknownField(usageField({ value: null })), true);
  assert.equal(isUnknownField(usageField({ value: "0" })), false);
});

// ── sumKnownBucketTotals ──────────────────────────────────────────────────────

function bucket(overrides = {}) {
  return {
    start: 1_700_000_000,
    end: 1_700_086_400,
    usage: reportedUsage(),
    reportCount: 0,
    hasUnknownUsage: false,
    ...overrides,
  };
}

test("sumKnownBucketTotals returns knownTotal null and partial false for an all-empty window", () => {
  const result = sumKnownBucketTotals([
    bucket({ reportCount: 0 }),
    bucket({ reportCount: 0 }),
  ]);
  assert.equal(result.kind, "unknown");
  assert.equal(result.value, null);
  assert.equal(result.partial, false);
});

test("sumKnownBucketTotals sums all known totals when every bucket is fully known", () => {
  const result = sumKnownBucketTotals([
    bucket({
      usage: reportedUsage({ totalTokens: usageField({ value: "100" }) }),
      reportCount: 1,
    }),
    bucket({
      usage: reportedUsage({ totalTokens: usageField({ value: "200" }) }),
      reportCount: 1,
    }),
  ]);
  assert.equal(result.kind, "exact");
  assert.equal(result.value, 300n);
  assert.equal(result.partial, false);
});

test("sumKnownBucketTotals marks partial true when any bucket has an incomplete (known lower-bound) total", () => {
  const result = sumKnownBucketTotals([
    bucket({
      usage: reportedUsage({
        totalTokens: usageField({ value: "100", incomplete: true }),
      }),
      reportCount: 1,
    }),
    bucket({
      usage: reportedUsage({ totalTokens: usageField({ value: "200" }) }),
      reportCount: 1,
    }),
  ]);
  assert.equal(result.kind, "exact");
  assert.equal(result.value, 300n);
  assert.equal(result.partial, true);
});

test("sumKnownBucketTotals preserves known exact subtotal when a sibling bucket is unknown (partial=true)", () => {
  // Thufir's explicit ruling: one unknown report-bearing bucket must NOT erase the known subtotal.
  // The result surfaces the labeled lower bound rather than hiding measured data.
  const result = sumKnownBucketTotals([
    bucket({
      usage: reportedUsage({ totalTokens: usageField({ value: "100" }) }),
      reportCount: 1,
    }),
    // report-bearing bucket with no total and no i/o — display is unknown, but does NOT erase sum
    bucket({ usage: reportedUsage(), reportCount: 1, hasUnknownUsage: true }),
  ]);
  assert.equal(result.kind, "exact");
  assert.equal(result.value, 100n);
  assert.equal(result.partial, true); // unknown sibling sets partial
});

test("sumKnownBucketTotals returns approximate from i/o sum when all bucket totals are null but i/o is known", () => {
  // Real-world case: no publisher emits totalTokens, but i/o are always present.
  const result = sumKnownBucketTotals([
    bucket({
      usage: reportedUsage({
        inputTokens: usageField({ value: "800" }),
        outputTokens: usageField({ value: "200" }),
      }),
      reportCount: 1,
    }),
    bucket({
      usage: reportedUsage({
        inputTokens: usageField({ value: "400" }),
        outputTokens: usageField({ value: "100" }),
      }),
      reportCount: 1,
    }),
  ]);
  assert.equal(result.kind, "approximate");
  assert.equal(result.value, 1500n); // (800+200) + (400+100)
  assert.equal(result.partial, false); // i/o fields are complete — no PARTIAL badge
});

test("sumKnownBucketTotals marks partial true when any approximate bucket has incomplete i/o", () => {
  const result = sumKnownBucketTotals([
    bucket({
      usage: reportedUsage({
        inputTokens: usageField({ value: "800", incomplete: true }),
        outputTokens: usageField({ value: "200" }),
      }),
      reportCount: 1,
    }),
    bucket({
      usage: reportedUsage({
        inputTokens: usageField({ value: "400" }),
        outputTokens: usageField({ value: "100" }),
      }),
      reportCount: 1,
    }),
  ]);
  assert.equal(result.kind, "approximate");
  assert.equal(result.partial, true);
});

test("sumKnownBucketTotals returns unknown when even i/o is unavailable for a report-bearing bucket", () => {
  const result = sumKnownBucketTotals([
    bucket({ usage: reportedUsage(), reportCount: 1, hasUnknownUsage: true }),
  ]);
  assert.equal(result.kind, "unknown");
  assert.equal(result.value, null);
  assert.equal(result.partial, false);
});

test("sumKnownBucketTotals returns approximate when mixed exact and approximate buckets exist (mixed provider support)", () => {
  // Steady-state once Task B supplies genuine totals for some providers:
  // one bucket has an exact total; another has null total but known i/o.
  const result = sumKnownBucketTotals([
    bucket({
      usage: reportedUsage({ totalTokens: usageField({ value: "1000" }) }),
      reportCount: 1,
    }),
    bucket({
      usage: reportedUsage({
        inputTokens: usageField({ value: "300" }),
        outputTokens: usageField({ value: "200" }),
      }),
      reportCount: 1,
    }),
  ]);
  // Any approximate bucket → aggregate is approximate; sums exact+approx display values.
  assert.equal(result.kind, "approximate");
  assert.equal(result.value, 1500n); // 1000 + (300+200)
  assert.equal(result.partial, false);
});

test("sumKnownBucketTotals preserves known approximate subtotal when a sibling bucket is unknown (partial=true)", () => {
  // Parallel to the exact+unknown case: approximate data from one bucket must not be erased.
  const result = sumKnownBucketTotals([
    bucket({
      usage: reportedUsage({
        inputTokens: usageField({ value: "400" }),
        outputTokens: usageField({ value: "100" }),
      }),
      reportCount: 1,
    }),
    // report-bearing bucket with no display value — sets partial, does NOT erase sum
    bucket({ usage: reportedUsage(), reportCount: 1, hasUnknownUsage: true }),
  ]);
  assert.equal(result.kind, "approximate");
  assert.equal(result.value, 500n);
  assert.equal(result.partial, true); // unknown sibling sets partial
});

test("sumKnownBucketTotals returns unknown when all report-bearing buckets have no display value", () => {
  const result = sumKnownBucketTotals([
    bucket({ usage: reportedUsage(), reportCount: 1, hasUnknownUsage: true }),
    bucket({ usage: reportedUsage(), reportCount: 1, hasUnknownUsage: true }),
  ]);
  assert.equal(result.kind, "unknown");
  assert.equal(result.value, null);
  assert.equal(result.partial, false);
});

// ── deriveUsageIngressTrailing ────────────────────────────────────────────────

function baseSeries(overrides = {}) {
  return {
    collectionEnabled: true,
    buckets: [],
    agents: [],
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

test("deriveUsageIngressTrailing returns 'Collection off' when collection is disabled", () => {
  const series = baseSeries({ collectionEnabled: false });
  assert.equal(deriveUsageIngressTrailing(series), "Collection off");
});

test("deriveUsageIngressTrailing returns 'No recent data' when collection is on but no agents present", () => {
  const series = baseSeries({ agents: [] });
  assert.equal(deriveUsageIngressTrailing(series), "No recent data");
});

test("deriveUsageIngressTrailing returns compact token count when a known non-partial total is available", () => {
  const series = baseSeries({
    agents: [agentUsage("a", "1500")],
  });
  assert.equal(deriveUsageIngressTrailing(series), "1.5K");
});

test("deriveUsageIngressTrailing appends '· Partial' when the total is a known lower bound", () => {
  const series = baseSeries({
    agents: [
      agentUsage("a", null, {
        usage: reportedUsage({
          totalTokens: usageField({ value: "1500", incomplete: true }),
        }),
      }),
    ],
  });
  assert.equal(deriveUsageIngressTrailing(series), "1.5K · Partial");
});

test("deriveUsageIngressTrailing returns 'Input/output reported' when only input or output is known", () => {
  const series = baseSeries({
    agents: [
      agentUsage("a", null, {
        usage: reportedUsage({
          inputTokens: usageField({ value: "800" }),
          outputTokens: usageField({ value: "200" }),
        }),
      }),
    ],
  });
  assert.equal(deriveUsageIngressTrailing(series), "Input/output reported");
});

test("deriveUsageIngressTrailing appends '· Partial' when only incomplete I/O fields are known", () => {
  const series = baseSeries({
    agents: [
      agentUsage("a", null, {
        usage: reportedUsage({
          inputTokens: usageField({ value: "800", incomplete: true }),
          outputTokens: usageField({ value: "200" }),
        }),
      }),
    ],
  });
  assert.equal(
    deriveUsageIngressTrailing(series),
    "Input/output reported · Partial",
  );
});

test("deriveUsageIngressTrailing returns 'No recent data' when all usage fields are unknown", () => {
  const series = baseSeries({
    agents: [agentUsage("a", null)],
  });
  assert.equal(deriveUsageIngressTrailing(series), "No recent data");
});

// ── Custom-range parsing, validation, and boundary construction ──────────────

test("parseLocalDate resolves a YYYY-MM-DD string to local midnight, not UTC midnight", () => {
  withTz("America/New_York", () => {
    const parsed = parseLocalDate("2026-03-15");
    assert.notEqual(parsed, null);
    // `new Date("2026-03-15")` is UTC midnight, which is Mar 14 20:00 in
    // New York — the field-wise parse must land on Mar 15 locally instead.
    assert.equal(parsed.getFullYear(), 2026);
    assert.equal(parsed.getMonth(), 2);
    assert.equal(parsed.getDate(), 15);
    assert.equal(parsed.getHours(), 0);
    assert.notEqual(parsed.getTime(), new Date("2026-03-15").getTime());
  });
});

test("parseLocalDate rejects a nonexistent calendar date instead of rolling it forward", () => {
  // `new Date(2026, 1, 30)` silently normalizes to Mar 2 — querying the
  // wrong civil day. The guard must reject it outright.
  assert.equal(parseLocalDate("2026-02-30"), null);
  assert.equal(parseLocalDate("2026-13-01"), null);
  assert.equal(parseLocalDate("2026-00-10"), null);
});

test("parseLocalDate rejects malformed input", () => {
  for (const value of [
    "",
    "2026-3-15",
    "15/03/2026",
    "2026-03-15T00:00",
    "x",
  ]) {
    assert.equal(parseLocalDate(value), null, `expected null for ${value}`);
  }
});

test("parseLocalDate accepts a leap day in a leap year and rejects it otherwise", () => {
  assert.notEqual(parseLocalDate("2024-02-29"), null);
  assert.equal(parseLocalDate("2026-02-29"), null);
});

test("formatLocalDate round-trips through parseLocalDate", () => {
  withTz("America/New_York", () => {
    for (const value of ["2026-01-01", "2026-03-08", "2026-12-31"]) {
      assert.equal(formatLocalDate(parseLocalDate(value)), value);
    }
  });
});

test("countRangeDays counts an inclusive single-day range as one day", () => {
  assert.equal(countRangeDays("2026-05-04", "2026-05-04"), 1);
});

test("countRangeDays counts civil days, not 24-hour spans, across a DST transition", () => {
  withTz("America/New_York", () => {
    // Mar 8 2026 is spring-forward: the span is 23h short of 3 * 24h but is
    // still 3 civil days.
    assert.equal(countRangeDays("2026-03-07", "2026-03-09"), 3);
  });
});

test("countRangeDays returns null for an inverted or malformed range", () => {
  assert.equal(countRangeDays("2026-05-10", "2026-05-01"), null);
  assert.equal(countRangeDays("nope", "2026-05-01"), null);
  assert.equal(countRangeDays("2026-05-01", "2026-02-30"), null);
});

test("countRangeDays stops counting past the cap instead of walking an absurd range", () => {
  const days = countRangeDays("1900-01-01", "2100-01-01");
  assert.ok(days > MAX_RANGE_DAYS, "over-cap range reports over-cap");
});

test("validateCustomRange accepts a range exactly at the maximum length", () => {
  // 2024 is a leap year: Jan 1 – Dec 31 inclusive is 366 civil days.
  const result = validateCustomRange("2024-01-01", "2024-12-31");
  assert.deepEqual(result, { ok: true, days: MAX_RANGE_DAYS });
});

test("validateCustomRange rejects a range one day past the maximum with a length message", () => {
  const result = validateCustomRange("2024-01-01", "2025-01-01");
  assert.equal(result.ok, false);
  assert.match(result.message, /366 days or fewer/);
});

test("validateCustomRange rejects an inverted range with an ordering message", () => {
  const result = validateCustomRange("2026-05-10", "2026-05-01");
  assert.equal(result.ok, false);
  assert.match(result.message, /on or before/);
});

test("validateCustomRange rejects a missing or malformed endpoint", () => {
  for (const [start, end] of [
    ["", "2026-05-01"],
    ["2026-05-01", ""],
    ["2026-02-30", "2026-05-01"],
  ]) {
    const result = validateCustomRange(start, end);
    assert.equal(result.ok, false);
    assert.match(result.message, /start and an end date/);
  }
});

test("buildCustomDayBoundaries returns days+1 strictly increasing boundaries closing the final day", () => {
  withTz("America/New_York", () => {
    const boundaries = buildCustomDayBoundaries("2026-05-01", "2026-05-03");
    assert.equal(boundaries.length, 4);
    assertStrictlyIncreasing(boundaries);
    assert.equal(
      boundaries[0],
      Math.floor(new Date(2026, 4, 1).getTime() / 1_000),
    );
    assert.equal(
      boundaries.at(-1),
      Math.floor(new Date(2026, 4, 4).getTime() / 1_000),
      "final boundary opens the day after the requested end date",
    );
  });
});

test("buildCustomDayBoundaries returns exactly 2 boundaries for a single-day range", () => {
  withTz("America/New_York", () => {
    const boundaries = buildCustomDayBoundaries("2026-05-04", "2026-05-04");
    assert.equal(boundaries.length, 2);
    assertStrictlyIncreasing(boundaries);
  });
});

test("buildCustomDayBoundaries stays strictly increasing across a spring-forward DST transition", () => {
  withTz("America/New_York", () => {
    const boundaries = buildCustomDayBoundaries("2026-03-06", "2026-03-10");
    assert.equal(boundaries.length, 6);
    assertStrictlyIncreasing(boundaries);
  });
});

test("buildCustomDayBoundaries emits no duplicate boundary across a skipped civil date", () => {
  withTz("Pacific/Apia", () => {
    // 2011-12-30 does not exist in Apia (date-line move). The walk must
    // produce distinct midnights rather than a duplicated boundary.
    const boundaries = buildCustomDayBoundaries("2011-12-28", "2011-12-31");
    assertStrictlyIncreasing(boundaries);
    assert.equal(new Set(boundaries).size, boundaries.length);
  });
});

test("buildCustomDayBoundaries produces the maximum boundary count at the cap", () => {
  withTz("America/New_York", () => {
    const boundaries = buildCustomDayBoundaries("2024-01-01", "2024-12-31");
    assert.equal(boundaries.length, MAX_RANGE_DAYS + 1);
    assertStrictlyIncreasing(boundaries);
  });
});

test("buildCustomDayBoundaries returns no boundaries for a range the picker rejects", () => {
  assert.deepEqual(buildCustomDayBoundaries("2026-05-10", "2026-05-01"), []);
  assert.deepEqual(buildCustomDayBoundaries("2024-01-01", "2025-01-01"), []);
  assert.deepEqual(buildCustomDayBoundaries("", ""), []);
});

test("buildRangeBoundaries matches buildLocalDayBoundaries for every preset", () => {
  const now = new Date(2026, 5, 15, 12, 0, 0);
  for (const days of [1, 7, 30]) {
    assert.deepEqual(
      buildRangeBoundaries({ kind: "preset", days }, now),
      buildLocalDayBoundaries(days, now),
      `preset ${days}d must not diverge from the shared day walk`,
    );
  }
});

test("buildRangeBoundaries yields 2 boundaries for the 1-day preset", () => {
  const now = new Date(2026, 5, 15, 12, 0, 0);
  const boundaries = buildRangeBoundaries({ kind: "preset", days: 1 }, now);
  assert.equal(boundaries.length, 2);
  assert.equal(
    boundaries[0],
    Math.floor(new Date(2026, 5, 15).getTime() / 1_000),
  );
  assert.equal(
    boundaries[1],
    Math.floor(new Date(2026, 5, 16).getTime() / 1_000),
  );
});

test("buildRangeBoundaries delegates custom ranges to the custom-day walk", () => {
  assert.deepEqual(
    buildRangeBoundaries({
      kind: "custom",
      startDate: "2026-05-01",
      endDate: "2026-05-03",
    }),
    buildCustomDayBoundaries("2026-05-01", "2026-05-03"),
  );
});

test("describeRange renders singular copy for the 1-day preset", () => {
  assert.equal(describeRange({ kind: "preset", days: 1 }), "the last day");
  assert.equal(describeRange({ kind: "preset", days: 7 }), "the last 7 days");
  assert.equal(describeRange({ kind: "preset", days: 30 }), "the last 30 days");
});

test("describeRange renders a custom range as a readable date span", () => {
  const described = describeRange({
    kind: "custom",
    startDate: "2026-05-01",
    endDate: "2026-05-03",
  });
  assert.match(described, /2026/);
  assert.match(described, /–/);
});
