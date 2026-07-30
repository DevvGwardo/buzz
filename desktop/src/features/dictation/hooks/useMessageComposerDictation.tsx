import type * as React from "react";
import { useRef } from "react";
import { useFeatureEnabled } from "@/shared/features";
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
  submitMessageRef: React.MutableRefObject<() => void>;
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
  return useComposerDictation({
    ...options,
    enabled,
    setEditorContentRef,
  });
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
