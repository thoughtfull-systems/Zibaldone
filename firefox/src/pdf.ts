import { type DownloadedAsset, fetchAsset } from "./assets";
import { downloadOrigins } from "./host";

const maxPdfBytes = 32 * 1024 * 1024;

function decode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function pdfName(asset: DownloadedAsset, tabTitle?: string): string {
  const header = asset.disposition ?? "";
  const encoded = /(?:^|;)\s*filename\*\s*=\s*UTF-8'[^']*'([^;]+)/i.exec(header)?.[1];
  const plain = /(?:^|;)\s*filename\s*=\s*(?:"([^"]*)"|([^;]*))/i.exec(header);
  const pathName = decode(new URL(asset.url).pathname.split("/").pop() ?? "");
  const name =
    (encoded && decode(encoded.trim())) ||
    (plain?.[1] ?? plain?.[2]) ||
    (/\.pdf$/i.test(pathName) ? pathName : tabTitle);
  const basename =
    name
      ?.split(/[\\/]/)
      .pop()
      ?.replace(/[\p{Cc}\s]+/gu, " ")
      .trim() || "Document";
  const stem = [...basename.replace(/\.pdf$/i, "")].slice(0, 1000).join("") || "Document";
  return `${stem}.pdf`;
}

export async function downloadPdf(tabUrl?: string, tabTitle?: string) {
  if (!tabUrl) throw new Error("Firefox could not capture this page.");
  const url = new URL(tabUrl);
  if (url.protocol === "file:") {
    throw new Error("Use Add files in Zib to save a local PDF.");
  }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) {
    throw new Error("Firefox could not capture this page.");
  }

  let asset: DownloadedAsset;
  try {
    asset = await fetchAsset(url.href, maxPdfBytes, 20_000, undefined, "PDF");
  } catch (error) {
    if (error instanceof TypeError && !(await browser.permissions.contains(downloadOrigins))) {
      throw new Error("Enable Allow web downloads in the clipper settings to save this PDF.");
    }
    throw error;
  }
  if (new TextDecoder().decode(asset.bytes.subarray(0, 5)) !== "%PDF-") {
    throw new Error("Firefox could not capture this page and its address did not return a PDF.");
  }
  return { bytes: asset.bytes, name: pdfName(asset, tabTitle), url: url.href };
}
