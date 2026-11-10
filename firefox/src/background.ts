import { type DownloadedAsset, fetchAsset } from "./assets";
import type { Capture, ImageReference } from "./content";
import { saveClip } from "./host";
import { downloadPdf } from "./pdf";

const active = new Set<number>();
const maxImageBytes = 8 * 1024 * 1024;
const maxImagePixels = 16 * 1024 * 1024;
const maxImages = 64;
const maxTotalBytes = 32 * 1024 * 1024;
const resizedJpegEdge = 2048;

interface Asset {
  data: string;
  note: string;
}

type Part = string | { alt: string; image: number };

function noteId(when: number): string {
  const digits = "0123456789abcdefghjkmnpqrstvwxyz";
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  let time = BigInt(when);
  for (let index = 5; index >= 0; index--) {
    bytes[index] = Number(time & 255n);
    time >>= 8n;
  }
  bytes[6] = (bytes[6] & 15) | 112;
  bytes[8] = (bytes[8] & 63) | 128;
  let value = bytes.reduce((number, byte) => (number << 8n) | BigInt(byte), 0n);
  let id = "";
  for (let index = 0; index < 26; index++) {
    id = digits[Number(value & 31n)] + id;
    value >>= 5n;
  }
  return id;
}

async function imageBytes(
  tabId: number,
  origin: string,
  url: string,
  maxBytes: number,
  timeout: number,
): Promise<Uint8Array<ArrayBuffer>> {
  let downloaded: DownloadedAsset;
  if (new URL(url).origin === origin) {
    const [injected] = await browser.scripting.executeScript<
      [string, number, number, string],
      Promise<DownloadedAsset>
    >({
      args: [url, maxBytes, timeout, origin],
      func: fetchAsset,
      target: { tabId },
    });
    if (!injected?.result) throw new Error("Image unavailable.");
    downloaded = await injected.result;
  } else {
    downloaded = await fetchAsset(url, maxBytes, timeout);
  }

  const { bytes, type } = downloaded;
  const bitmap = await createImageBitmap(new Blob([bytes], { type }));
  try {
    const jpeg = type === "image/jpeg";
    const oversized = bitmap.width * bitmap.height > maxImagePixels;
    if (bitmap.width > 8192 || bitmap.height > 8192 || (oversized && !jpeg)) {
      throw new Error("Image dimensions are too large");
    }
    const supported = ["image/gif", "image/jpeg", "image/png", "image/webp"];
    if (!oversized && type && supported.includes(type)) return bytes;
    const scale = oversized ? resizedJpegEdge / Math.max(bitmap.width, bitmap.height) : 1;
    const canvas = new OffscreenCanvas(
      Math.max(1, Math.round(bitmap.width * scale)),
      Math.max(1, Math.round(bitmap.height * scale)),
    );
    const drawing = canvas.getContext("2d");
    if (!drawing) throw new Error("Unable to convert image");
    drawing.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    const converted = await canvas.convertToBlob({
      quality: 0.9,
      type: jpeg ? "image/jpeg" : "image/png",
    });
    if (converted.size > maxBytes) throw new Error("Converted image is too large");
    return new Uint8Array(await converted.arrayBuffer());
  } finally {
    bitmap.close();
  }
}

function base64(bytes: Uint8Array): string {
  const chunks: string[] = [];
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    chunks.push(String.fromCharCode(...bytes.subarray(offset, offset + 8192)));
  }
  return btoa(chunks.join(""));
}

function fallback({ alt, url }: ImageReference): string {
  const label = (alt || "Source image").replace(/[\x21-\x2f\x3a-\x40\x5b-\x60\x7b-\x7e]/g, "\\$&");
  return /^https?:/.test(url) ? `[${label}](<${url.replaceAll(">", "%3E")}>)` : label;
}

async function collect(tabId: number, capture: Capture, when: number) {
  const images: Asset[] = [];
  const body: Part[] = [];
  const seen = new Map<string, number | null>();
  let available = maxTotalBytes;
  let missed = 0;
  const deadline = Date.now() + 20_000;
  const origin = new URL(capture.url).origin;
  for (const part of capture.parts) {
    if (typeof part === "string") {
      body.push(part);
      continue;
    }
    let index = seen.get(part.url);
    if (index === undefined) {
      index = null;
      const remaining = deadline - Date.now();
      if (part.url && images.length < maxImages && available > 0 && remaining > 0) {
        try {
          const bytes = await imageBytes(
            tabId,
            origin,
            part.url,
            Math.min(available, maxImageBytes),
            Math.min(remaining, 8000),
          );
          index = images.length;
          images.push({ data: base64(bytes), note: noteId(when) });
          available -= bytes.length;
        } catch {
          // A blocked or unusable image gets a source link in the article.
        }
      }
      seen.set(part.url, index);
    }
    if (index === null) {
      body.push(fallback(part));
      missed++;
    } else {
      body.push({ alt: part.alt, image: index });
    }
  }
  return { body, images, missed };
}

async function setStatus(
  tabId: number,
  badge: string,
  title: string,
  color: string | null = null,
): Promise<void> {
  // A save may finish after its tab has closed.
  await Promise.allSettled([
    browser.action.setBadgeBackgroundColor({ tabId, color }),
    browser.action.setBadgeText({ tabId, text: badge }),
    browser.action.setTitle({ tabId, title }),
  ]);
}

browser.action.onClicked.addListener(async (tab) => {
  const tabId = tab.id;
  if (tabId === undefined || active.has(tabId)) return;
  const when = Date.now();
  active.add(tabId);
  try {
    await setStatus(tabId, "…", "Saving to Zib…", "#666666");
    const { space } = await browser.storage.local.get("space");
    if (typeof space !== "string" || !space) {
      throw new Error("Choose a destination space in the add-on settings.");
    }
    const [injected] = await browser.scripting
      .executeScript<[], Capture>({ files: ["content.js"], target: { tabId } })
      .catch(() => []);
    const note = noteId(when);
    let missed = 0;
    let transcriptMissing = false;
    let request: object;
    if (injected?.result) {
      const capture = injected.result;
      transcriptMissing = capture.transcriptMissing ?? false;
      const article = await collect(tabId, capture, when);
      missed = article.missed;
      request = {
        clip: {
          authors: capture.authors,
          body: article.body,
          "captured-at": when,
          note,
          "published-on": capture.publishedOn,
          space,
          title: capture.title,
          url: capture.url,
        },
        images: article.images,
        type: "import-web-clip",
      };
    } else {
      const pdf = await downloadPdf(tab.url, tab.title);
      request = {
        data: base64(pdf.bytes),
        name: pdf.name,
        note,
        space,
        type: "import-web-pdf",
        url: pdf.url,
      };
    }
    await saveClip(request);
    if (transcriptMissing) {
      await setStatus(tabId, "!", "Saved to Zib without a transcript.");
    } else if (missed) {
      await setStatus(tabId, "!", `Saved to Zib with ${missed} image(s) unavailable.`);
    } else {
      await setStatus(tabId, "✓", "Saved to Zib.", "#217a39");
    }
  } catch (error) {
    await setStatus(tabId, "×", `Couldn't save to Zib. ${error}`);
  } finally {
    active.delete(tabId);
  }
});
