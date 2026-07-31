import { useQuery } from "@tanstack/react-query";
import { LockKeyhole } from "lucide-react";

import { useUserProfileQuery } from "@/features/profile/hooks";
import {
  canReadMessageEmbedSource,
  isMatchingMessageEmbedEvent,
  messageEmbedExcerpt,
  type MessageEmbedLink,
} from "@/features/messages/lib/messageEmbed";
import { getEventById } from "@/shared/api/tauri";
import type { Channel } from "@/shared/api/types";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { UserAvatar } from "@/shared/ui/UserAvatar";

export function MessageEmbed({
  channel,
  link,
  onOpen,
}: {
  channel: Channel | undefined;
  link: MessageEmbedLink;
  onOpen: () => void;
}) {
  const canRead = canReadMessageEmbedSource(channel);
  const eventQuery = useQuery({
    queryKey: ["message-embed", link.channelId, link.messageId],
    queryFn: () => getEventById(link.messageId),
    enabled: canRead,
    retry: false,
    staleTime: 60_000,
  });
  const event =
    canRead &&
    eventQuery.data &&
    isMatchingMessageEmbedEvent(eventQuery.data, link)
      ? eventQuery.data
      : null;
  // Never resolve an author profile until channel access and event-channel
  // integrity have both been established.
  const profileQuery = useUserProfileQuery(event?.pubkey);
  const profile = profileQuery.data;

  if (!canRead || eventQuery.isError || (eventQuery.data && !event)) {
    return (
      <div
        className="flex min-h-24 w-96 max-w-full items-center gap-3 border-l-2 border-border px-4 py-3 text-muted-foreground"
        data-message-embed="unavailable"
      >
        <LockKeyhole aria-hidden="true" className="h-4 w-4 shrink-0" />
        <span className="text-sm font-medium">
          Private or unavailable message
        </span>
      </div>
    );
  }

  if (!event) {
    return (
      <div
        className="min-h-24 w-96 max-w-full animate-pulse border-l-2 border-border px-4 py-3"
        data-message-embed="loading"
      >
        <span className="sr-only">Loading message preview</span>
        <div className="mb-3 h-5 w-36 rounded bg-muted" />
        <div className="h-4 w-full rounded bg-muted" />
        <div className="mt-2 h-4 w-2/3 rounded bg-muted" />
      </div>
    );
  }

  const displayName =
    profile?.displayName?.trim() || truncatePubkey(event.pubkey);
  const excerpt = messageEmbedExcerpt(event.content) || "Message has no text";

  return (
    <button
      type="button"
      aria-label={`Open message by ${displayName} in ${channel?.name ?? "channel"}`}
      className="group w-96 max-w-full cursor-pointer border-l-2 border-primary px-4 py-3 text-left transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      data-message-embed="resolved"
      onClick={onOpen}
    >
      <span className="mb-2 flex min-w-0 items-center gap-2">
        <UserAvatar
          avatarUrl={profile?.avatarUrl ?? null}
          className="shrink-0"
          displayName={displayName}
          fallbackDelayMs={0}
          size="xs"
        />
        <span className="min-w-0 truncate text-xs font-semibold text-foreground">
          {displayName}
        </span>
        <span aria-hidden="true" className="text-xs text-muted-foreground">
          ·
        </span>
        <span className="min-w-0 truncate text-xs text-muted-foreground">
          #{channel?.name}
        </span>
      </span>
      <span className="line-clamp-3 block text-sm leading-5 text-foreground">
        {excerpt}
      </span>
    </button>
  );
}
