export interface DownloadedAsset {
  bytes: Uint8Array<ArrayBuffer>;
  disposition: string | null;
  type?: string;
  url: string;
}

// Firefox also runs this function in the page's isolated content script context. Keep all
// dependencies inside it so same-origin images can use the browser's existing session.
export async function fetchAsset(
  url: string,
  maxBytes: number,
  timeout: number,
  origin?: string,
  kind: "image" | "PDF" = "image",
): Promise<DownloadedAsset> {
  if (origin && (location.origin !== origin || new URL(url).origin !== origin)) {
    throw new Error("The tab navigated while clipping.");
  }
  const response = await fetch(url, {
    credentials: kind === "PDF" ? "include" : origin ? "same-origin" : "omit",
    headers: kind === "PDF" ? { Accept: "application/pdf" } : undefined,
    redirect: origin ? "error" : "follow",
    referrerPolicy: origin ? "same-origin" : "no-referrer",
    signal: AbortSignal.timeout(timeout),
  });
  if (!response.ok || !response.body) {
    throw new Error(`Couldn't download ${kind} (HTTP ${response.status}).`);
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.length;
      if (total > maxBytes) throw new Error(`${kind} exceeds ${maxBytes / 1024 ** 2} MiB.`);
      chunks.push(value);
    }
  } finally {
    await reader.cancel();
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return {
    bytes,
    disposition: response.headers.get("content-disposition"),
    type: response.headers.get("content-type")?.split(";", 1)[0].trim(),
    url: response.url,
  };
}
