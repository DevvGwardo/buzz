import assert from "node:assert/strict";
import test from "node:test";

import {
  getDictationSendDecision,
  replaceTrailingTranscribedText,
  shouldAutoSubmitDictation,
} from "./voiceInput.ts";

test("dictation send stops capture before submitting", () => {
  assert.equal(
    getDictationSendDecision({
      isRecording: true,
      isStarting: false,
      isTranscribing: true,
    }),
    "stop-recording",
  );
  assert.equal(
    getDictationSendDecision({
      isRecording: false,
      isStarting: true,
      isTranscribing: false,
    }),
    "stop-recording",
  );
});

test("dictation send waits for the final transcript", () => {
  assert.equal(
    getDictationSendDecision({
      isRecording: false,
      isStarting: false,
      isTranscribing: true,
    }),
    "wait",
  );
  assert.equal(
    getDictationSendDecision({
      isRecording: false,
      isStarting: false,
      isTranscribing: false,
    }),
    "send",
  );
});

test("dictation auto-submit waits until capture and transcription settle", () => {
  const state = {
    requested: true,
    isRecording: false,
    isStarting: false,
    isTranscribing: false,
  };
  assert.equal(shouldAutoSubmitDictation(state), true);
  assert.equal(
    shouldAutoSubmitDictation({ ...state, requested: false }),
    false,
  );
  assert.equal(
    shouldAutoSubmitDictation({ ...state, isRecording: true }),
    false,
  );
  assert.equal(
    shouldAutoSubmitDictation({ ...state, isStarting: true }),
    false,
  );
  assert.equal(
    shouldAutoSubmitDictation({ ...state, isTranscribing: true }),
    false,
  );
});

// ── replaceTrailingTranscribedText ──────────────────────────────────────────

test("replaceTrailingTranscribedText_appendsWhenNoPrevious", () => {
  assert.equal(
    replaceTrailingTranscribedText("Hello", "", "world"),
    "Hello world",
  );
});

test("replaceTrailingTranscribedText_appendsToEmptyBase", () => {
  assert.equal(replaceTrailingTranscribedText("", "", "hello"), "hello");
});

test("replaceTrailingTranscribedText_replacesTrailingInterim", () => {
  // Interim "hello wor" is refined to "hello world".
  assert.equal(
    replaceTrailingTranscribedText("hello wor", "hello wor", "hello world"),
    "hello world",
  );
});

test("replaceTrailingTranscribedText_preservesTextTypedBeforeDictation", () => {
  // User typed "Note: " then dictated; the manual prefix must survive.
  assert.equal(
    replaceTrailingTranscribedText("Note: hi", "hi", "hi there"),
    "Note: hi there",
  );
});

test("replaceTrailingTranscribedText_appendsWhenPreviousNoLongerMatches", () => {
  // If the previous transcript isn't the trailing text anymore, append.
  assert.equal(
    replaceTrailingTranscribedText("edited text", "old", "new"),
    "edited text new",
  );
});

test("replaceTrailingTranscribedText_noDoubleSpaceBeforePunctuation", () => {
  assert.equal(
    replaceTrailingTranscribedText("Hello", "", ", world"),
    "Hello, world",
  );
});
