declare const ZIB_NATIVE_HOST: string;

export interface Space {
  id: string;
  name: string;
}

export interface SavedClip {
  images: string[];
  note: string;
  space: string;
}

type HostResult = { clip: SavedClip; ok: true } | { ok: true; spaces: Space[] };

export type HostReply = HostResult | { error: string; ok: false };

export async function send(request: object): Promise<HostReply> {
  return browser.runtime.sendNativeMessage(ZIB_NATIVE_HOST, request) as Promise<HostReply>;
}

export const downloadOrigins = { origins: ["http://*/*", "https://*/*"] };

export async function saveClip(request: object): Promise<void> {
  // Retry a disconnected host once using exactly the same note and image identifiers.
  const reply = await send(request).catch(() => send(request));
  if (!reply.ok) throw new Error(reply.error);
  if (!("clip" in reply)) throw new Error("Zib returned an unexpected reply.");
}
