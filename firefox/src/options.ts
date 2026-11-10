import { downloadOrigins, send } from "./host";

function element<T extends HTMLElement>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`Missing settings element: ${selector}`);
  return found;
}

const picker = element<HTMLSelectElement>("#space");
const reload = element<HTMLButtonElement>("#reload");
const status = element<HTMLElement>("#status");
const downloadAccess = element<HTMLButtonElement>("#download-access");
const downloadStatus = element<HTMLElement>("#download-status");

async function loadSpaces(): Promise<void> {
  picker.disabled = true;
  reload.disabled = true;
  status.textContent = "Connecting to Zib…";
  try {
    const reply = await send({ type: "spaces" });
    if (!reply.ok) throw new Error(reply.error);
    if (!("spaces" in reply)) throw new Error("Zib returned an unexpected reply.");
    const { space } = await browser.storage.local.get("space");
    const counts = new Map<string, number>();
    for (const entry of reply.spaces) counts.set(entry.name, (counts.get(entry.name) ?? 0) + 1);
    picker.replaceChildren(new Option("Choose a space", ""));
    for (const { id, name } of reply.spaces) {
      picker.add(new Option(counts.get(name) === 1 ? name : `${name} (${id})`, id));
    }
    picker.value = reply.spaces.some((entry) => entry.id === space) ? space : "";
    picker.disabled = reply.spaces.length === 0;
    status.textContent = picker.disabled
      ? "Create a space in Zib, then refresh."
      : space && !picker.value
        ? "The saved destination is unavailable. Choose a space."
        : picker.value
          ? "Ready to clip."
          : "Choose where to save clips.";
  } catch (error) {
    status.textContent = `Couldn't list spaces. Check the Zib native host. ${error}`;
  } finally {
    reload.disabled = false;
  }
}

async function showDownloadAccess(): Promise<void> {
  const allowed = await browser.permissions.contains(downloadOrigins);
  downloadAccess.textContent = allowed ? "Remove web download access" : "Allow web downloads";
  downloadAccess.dataset.allowed = String(allowed);
  downloadStatus.textContent = allowed ? "PDF and cross-site image downloads are enabled." : "";
}

picker.addEventListener("change", async () => {
  await browser.storage.local.set({ space: picker.value });
  status.textContent = picker.value ? "Ready to clip." : "Choose where to save clips.";
});
reload.addEventListener("click", () => void loadSpaces());
downloadAccess.addEventListener("click", async () => {
  try {
    if (downloadAccess.dataset.allowed === "true") {
      await browser.permissions.remove(downloadOrigins);
    } else {
      await browser.permissions.request(downloadOrigins);
    }
    await showDownloadAccess();
  } catch (error) {
    downloadStatus.textContent = String(error);
  }
});
void loadSpaces();
void showDownloadAccess();
