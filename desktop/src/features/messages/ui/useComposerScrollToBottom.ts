import * as React from "react";

export function useComposerScrollToBottom(
  composerScrollRef: React.RefObject<HTMLDivElement | null>,
) {
  return React.useCallback(() => {
    window.requestAnimationFrame(() => {
      const scrollElement = composerScrollRef.current;
      if (!scrollElement) return;
      scrollElement.scrollTop = scrollElement.scrollHeight;
    });
  }, [composerScrollRef]);
}
