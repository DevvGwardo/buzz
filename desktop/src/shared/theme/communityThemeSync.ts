import { relayClient } from "@/shared/api/relayClient";
import {
  nip44DecryptFromSelf,
  nip44EncryptToSelf,
  signRelayEvent,
} from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_COMMUNITY_THEME } from "@/shared/constants/kinds";
import {
  parseCommunityThemePreference,
  sameCommunityThemePreference,
  type CommunityThemePreference,
} from "./communityThemePreference";

const D_TAG = "community-theme";
const DEBOUNCE_MS = 2_000;

export type RemoteCommunityTheme = {
  preference: CommunityThemePreference;
  createdAt: number;
  eventId: string;
};

export type RemoteCommunityThemeResult =
  | { status: "valid"; remote: RemoteCommunityTheme }
  | { status: "absent" | "invalid" | "unavailable" };

export function shouldSeedCommunityTheme(
  result: RemoteCommunityThemeResult,
): boolean {
  return result.status === "absent";
}

async function decryptAndParse(
  event: RelayEvent,
): Promise<RemoteCommunityTheme | null> {
  try {
    const plaintext = await nip44DecryptFromSelf(event.content);
    const preference = parseCommunityThemePreference(JSON.parse(plaintext));
    return preference
      ? { preference, createdAt: event.created_at, eventId: event.id }
      : null;
  } catch {
    return null;
  }
}

export class CommunityThemeSyncManager {
  private readonly pubkey: string;
  private debounceTimer: number | null = null;
  private destroyed = false;
  private lastRemoteCreatedAt = 0;
  private lastPublished: CommunityThemePreference | null = null;
  private pending: CommunityThemePreference | null = null;

  constructor(pubkey: string) {
    this.pubkey = pubkey;
  }

  async fetchRemote(): Promise<RemoteCommunityThemeResult> {
    try {
      const events = await relayClient.fetchEvents({
        kinds: [KIND_COMMUNITY_THEME],
        authors: [this.pubkey],
        "#d": [D_TAG],
        limit: 1,
      });
      if (events.length === 0) return { status: "absent" };
      if (events[0].pubkey !== this.pubkey) return { status: "invalid" };
      const remote = await decryptAndParse(events[0]);
      if (!remote) return { status: "invalid" };
      this.lastRemoteCreatedAt = Math.max(
        this.lastRemoteCreatedAt,
        remote.createdAt,
      );
      return { status: "valid", remote };
    } catch {
      return { status: "unavailable" };
    }
  }

  publish(preference: CommunityThemePreference): void {
    if (this.destroyed) return;
    this.pending = preference;
    if (this.debounceTimer !== null) {
      window.clearTimeout(this.debounceTimer);
    }
    this.debounceTimer = window.setTimeout(() => {
      this.debounceTimer = null;
      void this.doPublish(preference);
    }, DEBOUNCE_MS);
  }

  getPending(): CommunityThemePreference | null {
    return this.pending;
  }

  cancelPendingPublish(): void {
    if (this.debounceTimer !== null) {
      window.clearTimeout(this.debounceTimer);
      this.debounceTimer = null;
    }
    this.pending = null;
  }

  private async doPublish(preference: CommunityThemePreference): Promise<void> {
    try {
      if (
        this.destroyed ||
        (this.lastPublished &&
          sameCommunityThemePreference(this.lastPublished, preference))
      ) {
        this.pending = null;
        return;
      }
      const ciphertext = await nip44EncryptToSelf(JSON.stringify(preference));
      if (this.destroyed) return;
      const event = await signRelayEvent({
        kind: KIND_COMMUNITY_THEME,
        content: ciphertext,
        createdAt: Math.max(
          Math.floor(Date.now() / 1_000),
          this.lastRemoteCreatedAt + 1,
        ),
        tags: [
          ["d", D_TAG],
          ["t", D_TAG],
        ],
      });
      if (this.destroyed) return;
      await relayClient.publishEvent(
        event,
        "Timed out publishing community theme.",
        "Failed to publish community theme.",
      );
      this.lastRemoteCreatedAt = event.created_at;
      this.lastPublished = preference;
      this.pending = null;
    } catch (error) {
      console.warn("[communityThemeSync] publish failed:", error);
    }
  }

  async subscribe(
    onUpdate: (remote: RemoteCommunityTheme) => void,
  ): Promise<() => Promise<void>> {
    return relayClient.subscribeLive(
      {
        kinds: [KIND_COMMUNITY_THEME],
        authors: [this.pubkey],
        "#d": [D_TAG],
        limit: 0,
      },
      (event: RelayEvent) => {
        if (event.pubkey !== this.pubkey || this.destroyed) return;
        void decryptAndParse(event).then((remote) => {
          if (!remote || this.destroyed) return;
          this.lastRemoteCreatedAt = Math.max(
            this.lastRemoteCreatedAt,
            remote.createdAt,
          );
          onUpdate(remote);
        });
      },
    );
  }

  destroy(): void {
    this.destroyed = true;
    this.cancelPendingPublish();
  }
}
