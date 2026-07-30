import { useCallback, useRef } from "react";
import { replaceTrailingTranscribedText } from "../lib/voiceInput";
import { useLocalDictation } from "./useLocalDictation";

interface UseDictationOptions {
  /** Disable native availability checks and recording entry points. */
  disabled?: boolean;
  /** Returns the current composer text (must be fresh — synced from editor). */
  getText: () => string;
  /** Set composer text */
  setText: (value: string) => void;
}

export function useDictation({
  disabled = false,
  getText,
  setText,
}: UseDictationOptions) {
  const lastTranscriptRef = useRef("");

  const handleTranscript = useCallback(
    (transcript: string) => {
      const previous = lastTranscriptRef.current;
      const latest = getText();
      const merged = replaceTrailingTranscribedText(
        latest,
        previous,
        transcript,
      );
      setText(merged);
      // Each native flush is an independent segment, so the next transcript
      // appends instead of replacing this one.
      lastTranscriptRef.current = "";
    },
    [getText, setText],
  );

  const dictation = useLocalDictation({
    disabled,
    onRecordingStart: () => {
      lastTranscriptRef.current = "";
    },
    onTranscriptText: handleTranscript,
  });

  return dictation;
}
