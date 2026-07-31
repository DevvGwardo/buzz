import * as React from "react";

import { useThreadActivityFeedItems } from "@/app/useThreadActivityFeedItems";
import { msgContextKey } from "@/features/channels/readState/readStateFormat";
import type { ThreadActivityItem } from "@/features/channels/useUnreadChannels";
import { isThreadReply } from "@/features/messages/lib/threading";
import type { Channel, FeedItem, HomeFeed } from "@/shared/api/types";

type ReadTimestamp = (contextKey: string) => number | null;
type MarkChannelRead = (
  contextKey: string,
  readAt: string | null | undefined,
  options?: { topLevelOnly?: boolean },
) => void;

type UseChannelActivityProjectionOptions = {
  channels: Channel[];
  feed: HomeFeed | undefined;
  unreadFeedItemIds: ReadonlySet<string>;
  getChannelReadAt: ReadTimestamp;
  getOwnReadAt: ReadTimestamp;
  markChannelRead: MarkChannelRead;
  readStateVersion: number;
  threadActivityItems: ThreadActivityItem[];
  mutedRootIds: ReadonlySet<string>;
};

export function useChannelActivityProjection({
  channels,
  feed,
  unreadFeedItemIds,
  getChannelReadAt,
  getOwnReadAt,
  markChannelRead,
  readStateVersion,
  threadActivityItems,
  mutedRootIds,
}: UseChannelActivityProjectionOptions) {
  const getThreadReadAt = React.useCallback(
    (rootId: string, channelId?: string | null) => {
      const threadReadAt = getOwnReadAt(`thread:${rootId}`);
      if (!channelId) return threadReadAt;

      const channelReadAt = getChannelReadAt(channelId);
      if (threadReadAt === null) return channelReadAt;
      if (channelReadAt === null) return threadReadAt;
      return Math.max(threadReadAt, channelReadAt);
    },
    [getChannelReadAt, getOwnReadAt],
  );
  const markThreadRead = React.useCallback(
    (rootId: string, timestamp: number) =>
      markChannelRead(
        `thread:${rootId}`,
        new Date(timestamp * 1_000).toISOString(),
      ),
    [markChannelRead],
  );
  const getMessageReadAt = React.useCallback(
    (messageId: string) => getChannelReadAt(msgContextKey(messageId)),
    [getChannelReadAt],
  );
  const markMessageRead = React.useCallback(
    (messageId: string, timestamp: number) =>
      markChannelRead(
        msgContextKey(messageId),
        new Date(timestamp * 1_000).toISOString(),
      ),
    [markChannelRead],
  );
  const threadActivityFeedItems = useThreadActivityFeedItems(
    threadActivityItems,
    mutedRootIds,
    channels,
  );
  const locallyUnreadFeedItems = React.useMemo(() => {
    if (!feed || unreadFeedItemIds.size === 0) return [];
    return [
      ...feed.mentions,
      ...feed.needsAction,
      ...feed.activity,
      ...feed.agentActivity,
    ].filter((item) => unreadFeedItemIds.has(item.id));
  }, [feed, unreadFeedItemIds]);
  const unreadThreadFeedItems = React.useMemo(() => {
    void readStateVersion;
    const candidatesById = new Map<string, FeedItem>(
      threadActivityFeedItems.map((item) => [item.id, item]),
    );
    for (const item of locallyUnreadFeedItems)
      candidatesById.set(item.id, item);

    return [...candidatesById.values()].filter(
      (item) =>
        isThreadReply(item.tags) &&
        (unreadFeedItemIds.has(item.id) ||
          item.createdAt > (getMessageReadAt(item.id) ?? 0)),
    );
  }, [
    getMessageReadAt,
    locallyUnreadFeedItems,
    readStateVersion,
    threadActivityFeedItems,
    unreadFeedItemIds,
  ]);
  const unreadThreadChannelIds = React.useMemo(
    () =>
      new Set(
        unreadThreadFeedItems.flatMap((item) =>
          item.channelId ? [item.channelId] : [],
        ),
      ) as ReadonlySet<string>,
    [unreadThreadFeedItems],
  );

  return {
    getThreadReadAt,
    markThreadRead,
    getMessageReadAt,
    markMessageRead,
    threadActivityFeedItems,
    locallyUnreadFeedItems,
    unreadThreadFeedItems,
    unreadThreadChannelIds,
  };
}
