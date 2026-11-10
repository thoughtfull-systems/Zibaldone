import Defuddle, { createMarkdownContent } from "defuddle/full";

export interface ImageReference {
  alt: string;
  url: string;
}

export interface Capture {
  authors: string[];
  parts: (string | ImageReference)[];
  publishedOn?: string;
  title: string;
  url: string;
}

function httpUrl(value: string, base: string): string | undefined {
  try {
    const url = new URL(value, base);
    if (["http:", "https:"].includes(url.protocol) && !url.username && !url.password) {
      return url.href;
    }
  } catch {
    // The page supplied a malformed link.
  }
}

function imageUrl(value: string, base: string): string | undefined {
  if (!value.trim()) return;
  if (/^data:image\//i.test(value) && value.length <= 8 * 1024 * 1024) return value;
  try {
    const url = new URL(value, base);
    if (url.protocol === "blob:" && url.origin === location.origin) return url.href;
  } catch {
    return;
  }
  return httpUrl(value, base);
}

function authorsFromPage(fallback: string): string[] {
  const names = Array.from(
    document.querySelectorAll<HTMLMetaElement>(
      'meta[name="author"], meta[name="citation_author"], meta[property="article:author"]',
    ),
    (meta) => meta.content.trim(),
  ).filter((name) => name && !/^https?:/i.test(name));
  return [...new Set(names.length ? names : [fallback.trim()].filter(Boolean))]
    .filter((name) => !/^https?:/i.test(name) && new TextEncoder().encode(name).length <= 1024)
    .slice(0, 64);
}

function calendarDate(value: string): string | undefined {
  const match = /^(\d{4}-\d{2}-\d{2})(?:$|T)/.exec(value.trim());
  if (!match) return;
  const date = new Date(`${match[1]}T00:00:00Z`);
  if (Number.isFinite(date.valueOf()) && date.toISOString().startsWith(match[1])) return match[1];
}

function sourceOf(image: HTMLImageElement): string {
  const lazy = image.getAttribute("data-src") ?? image.getAttribute("data-original");
  const responsive = image.getAttribute("data-srcset") ?? image.getAttribute("srcset");
  return lazy || image.getAttribute("src") || responsive?.split(/[ ,]/, 1)[0] || "";
}

export function capture(): Capture {
  const url = httpUrl(location.href, location.href);
  if (!url) throw new Error("Open an HTTP or HTTPS page to clip it.");
  const base = document.baseURI;
  const tabTitle = document.title.trim();
  const currentSources = new Map<string, string>();
  for (const image of document.images) {
    if (image.currentSrc && image.complete && image.naturalWidth) {
      const source = imageUrl(sourceOf(image), base);
      const src = imageUrl(image.getAttribute("src") ?? "", base);
      if (source && (source === src || source === image.currentSrc || image.srcset)) {
        currentSources.set(source, image.currentSrc);
      }
    }
  }

  const article = new Defuddle(document, {
    url,
    useAsync: false,
    includeReplies: false,
  }).parse();
  const fragment = document.createElement("template");
  fragment.innerHTML = article.content;
  for (const node of fragment.content.querySelectorAll("script, style, iframe, object, embed")) {
    node.remove();
  }
  for (const anchor of fragment.content.querySelectorAll("a[href]")) {
    const href = httpUrl(anchor.getAttribute("href") ?? "", base);
    if (href) anchor.setAttribute("href", href);
    else anchor.removeAttribute("href");
  }

  const random = crypto.getRandomValues(new Uint32Array(4));
  const marker = `ZIBCLIP${random.join("X")}X`;
  // Defuddle handles ordinary Markdown escapes. Mark prose dollars for Zib's math syntax.
  const dollar = `${marker}DOLLAR`;
  const verbatim = "code, pre, math, .katex, .math, .mwe-math-element";
  const text = document.createTreeWalker(fragment.content, NodeFilter.SHOW_TEXT);
  for (let node = text.nextNode(); node instanceof Text; node = text.nextNode()) {
    if (node.data.includes("$") && !node.parentElement?.closest(verbatim)) {
      node.data = node.data.replaceAll("$", () => dollar);
    }
  }

  // Replace image nodes before Markdown conversion so code and ordinary links stay untouched.
  const images: ImageReference[] = [];
  for (const image of fragment.content.querySelectorAll("img")) {
    const source = imageUrl(sourceOf(image), base);
    const actual = source && imageUrl(currentSources.get(source) ?? source, base);
    const alt = [...image.alt.trim().replace(/\s+/g, " ")].slice(0, 1024).join("");
    const token = `${marker}${images.length}END`;
    images.push({ alt, url: actual ?? "" });
    const parent = image.parentElement;
    if (parent?.tagName === "A" && parent.children.length === 1 && !parent.textContent?.trim()) {
      parent.replaceWith(token);
    } else {
      image.replaceWith(token);
    }
  }

  const markdown = createMarkdownContent(fragment.innerHTML, url).replaceAll(dollar, () => "\\$");
  const parts: Capture["parts"] = [];
  let offset = 0;
  for (const match of markdown.matchAll(new RegExp(`${marker}(\\d+)END`, "g"))) {
    if (match.index > offset) parts.push(markdown.slice(offset, match.index));
    parts.push(images[Number(match[1])]);
    offset = match.index + match[0].length;
  }
  if (offset < markdown.length) parts.push(markdown.slice(offset));
  if (!parts.length || !markdown.trim()) throw new Error("No article content was found.");
  return {
    authors: authorsFromPage(article.author ?? ""),
    parts,
    publishedOn: calendarDate(article.published ?? ""),
    title: [...(article.title?.trim() || tabTitle || new URL(url).hostname)]
      .slice(0, 1024)
      .join(""),
    url,
  };
}
