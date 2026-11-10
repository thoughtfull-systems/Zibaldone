import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { build } from "esbuild";

const channel = process.env.ZIB_CLIPPER_CHANNEL ?? "dev";
if (channel !== "dev" && channel !== "production") {
  throw new Error("ZIB_CLIPPER_CHANNEL must be dev or production");
}
const dev = channel === "dev";
const suffix = dev ? "-dev" : "";
const name = dev ? "Zib Web Clipper Dev" : "Zib Web Clipper";
const host = dev ? "systems.thoughtfull.zibaldone.dev" : "systems.thoughtfull.zibaldone";
const iconName = `systems.thoughtfull.Zibaldone${dev ? ".Devel" : ""}.svg`;
const iconSource = new URL(`../linux/data/icons/${iconName}`, import.meta.url);
const { version } = JSON.parse(await readFile(new URL("package.json", import.meta.url), "utf8"));
const manifest = {
  action: { default_icon: "icon.svg", default_title: name },
  background: { scripts: ["background.js"] },
  browser_specific_settings: {
    gecko: {
      data_collection_permissions: { required: ["browsingActivity", "websiteContent"] },
      id: `web-clipper${suffix}@zibaldone.thoughtfull.systems`,
      strict_min_version: "140.0",
    },
  },
  description: "Save articles, PDFs, and YouTube transcripts to your local Zibaldone notebook.",
  icons: { 48: "icon.svg" },
  manifest_version: 3,
  name,
  optional_host_permissions: ["http://*/*", "https://*/*"],
  options_ui: { open_in_tab: true, page: "options.html" },
  permissions: ["activeTab", "nativeMessaging", "scripting", "storage"],
  version,
};
const options = {
  bundle: true,
  define: { ZIB_NATIVE_HOST: JSON.stringify(host) },
  format: "iife",
  legalComments: "linked",
  outdir: "dist",
  target: "firefox128",
};
await mkdir("dist", { recursive: true });
await Promise.all([
  build({ ...options, entryPoints: ["src/background.ts", "src/options.ts"] }),
  build({
    ...options,
    entryPoints: ["src/content.ts"],
    footer: { js: "ZibCapture.capture();" },
    globalName: "ZibCapture",
  }),
  copyFile(iconSource, "dist/icon.svg"),
  copyFile("options.html", "dist/options.html"),
  copyFile("node_modules/defuddle/LICENSE", "dist/Defuddle-LICENSE.txt"),
  copyFile("node_modules/mathml-to-latex/LICENSE.md", "dist/MathML-to-LaTeX-LICENSE.txt"),
  copyFile("node_modules/temml/LICENSE", "dist/Temml-LICENSE.txt"),
  copyFile("node_modules/turndown/LICENSE", "dist/Turndown-LICENSE.txt"),
  copyFile("node_modules/@xmldom/xmldom/LICENSE", "dist/xmldom-LICENSE.txt"),
  writeFile("dist/manifest.json", `${JSON.stringify(manifest, null, 2)}\n`),
]);
