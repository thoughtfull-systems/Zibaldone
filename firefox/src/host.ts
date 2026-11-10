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
