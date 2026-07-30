function appendTranscribedText(baseText: string, fragment: string): string {
  const normalizedFragment = fragment.replace(/\s+/g, " ").trim();
  if (!normalizedFragment) return baseText;
  if (!baseText.trim()) return normalizedFragment;
  if (/[\s([{/-]$/.test(baseText) || /^[,.;!?)]/.test(normalizedFragment)) {
    return `${baseText}${normalizedFragment}`;
  }
  return `${baseText} ${normalizedFragment}`;
}

export type DictationSendDecision = "send" | "stop-recording" | "wait";

export function getDictationSendDecision({
  isRecording,
  isStarting,
  isTranscribing,
}: {
  isRecording: boolean;
  isStarting: boolean;
  isTranscribing: boolean;
}): DictationSendDecision {
  if (isRecording || isStarting) return "stop-recording";
  if (isTranscribing) return "wait";
  return "send";
}

export function replaceTrailingTranscribedText(
  fullText: string,
  previousTranscribedText: string,
  nextTranscribedText: string,
): string {
  if (!previousTranscribedText) {
    return appendTranscribedText(fullText, nextTranscribedText);
  }

  if (fullText.endsWith(previousTranscribedText)) {
    return appendTranscribedText(
      fullText.slice(0, -previousTranscribedText.length),
      nextTranscribedText,
    );
  }

  const trimmedPreviousText = previousTranscribedText.trim();
  if (trimmedPreviousText && fullText.endsWith(trimmedPreviousText)) {
    return appendTranscribedText(
      fullText.slice(0, -trimmedPreviousText.length),
      nextTranscribedText,
    );
  }

  return appendTranscribedText(fullText, nextTranscribedText);
}
