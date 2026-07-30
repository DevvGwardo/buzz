import type * as React from "react";
import { useCallback, useRef } from "react";
import { useFeatureEnabled } from "@/shared/features";
import { getDictationSendDecision } from "../lib/voiceInput";
import { DictationButton } from "../ui/DictationButton";
import { useComposerDictation } from "./useComposerDictation";

interface UseMessageComposerDictationOptions {
  syncContentRef: React.MutableRefObject<() => string>;
  disabled: boolean;
  disabledRef: React.MutableRefObject<boolean>;
  isSendingRef: React.MutableRefObject<boolean>;
  isUploadingRef: React.MutableRefObject<boolean>;
  setComposerContent: (text: string) => void;
  setEditorContent: (text: string) => void;
  draftKey: string | null;
  composerRef: React.RefObject<HTMLElement | null>;
}

export function useMessageComposerDictation({
  setEditorContent,
  ...options
}: UseMessageComposerDictationOptions) {
  const enabled = useFeatureEnabled("voiceDictation");
  const setEditorContentRef = useRef(setEditorContent);
  setEditorContentRef.current = setEditorContent;
  const dictation = useComposerDictation({
    ...options,
    enabled,
    setEditorContentRef,
  });
  const { isRecording, isStarting, isTranscribing, stopRecording } = dictation;
  const prepareToSubmit = useCallback(() => {
    const decision = getDictationSendDecision({
      isRecording,
      isStarting,
      isTranscribing,
    });
    if (decision === "stop-recording") {
      stopRecording();
    }
    return decision === "send";
  }, [isRecording, isStarting, isTranscribing, stopRecording]);

  return {
    ...dictation,
    prepareToSubmit,
  };
}

export function MessageComposerDictationAction({
  children,
  dictation,
  disabled,
}: {
  children?: React.ReactNode;
  dictation: ReturnType<typeof useMessageComposerDictation>;
  disabled: boolean;
}) {
  return (
    <>
      <DictationButton dictation={dictation} disabled={disabled} />
      {children}
    </>
  );
}
