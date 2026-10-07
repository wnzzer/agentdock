import type { Attachment } from "./attachment-state";
import { MAX_ATTACHMENT_BYTES } from "./attachment-state";
import { mediaKind } from "./media-kind";

/**
 * Files pasted or dropped into a terminal.
 *
 * The program in the terminal runs on the host and cannot see the browser's
 * clipboard, so the file is uploaded into the session's working directory and
 * its path is pasted instead -- what dragging a file into a local terminal
 * does. Claude Code and Codex both turn a pasted image path into an image
 * attachment; any other program gets an ordinary path.
 */

/** Characters a shell (and these clients' path detection) needs escaped, as a terminal escapes a dropped file. */
const SHELL_SPECIAL = /([\s'"\\$`!&*()|;<>?#~{}[\]])/g;
export function shellPath(path: string): string {
  return path.replace(SHELL_SPECIAL, "\\$1");
}

/** What a set of uploads pastes: escaped paths separated by spaces, with a trailing space to keep typing after. */
export function pasteText(paths: string[]): string {
  return paths.length ? paths.map(shellPath).join(" ") + " " : "";
}

const EXTENSIONS: Record<string, string> = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp", "image/heic": "heic", "image/heif": "heif", "image/avif": "avif", "image/bmp": "bmp", "image/tiff": "tiff", "application/pdf": "pdf", "text/plain": "txt" };
const pad = (value: number) => String(value).padStart(2, "0");

/**
 * The name a file is stored under. A clipboard image arrives as "image.png"
 * (or nameless), so every screenshot would look alike; those are named for
 * when they were pasted instead. A real file keeps its own name.
 */
export function uploadName(file: { name: string; type: string }, now = new Date()): string {
  const generic = !file.name || /^image\.(png|jpe?g|gif|webp|heic|tiff?)$/i.test(file.name) || /^blob$/i.test(file.name);
  if (!generic) return file.name;
  const extension = EXTENSIONS[file.type] ?? (file.name.split(".").pop() || "bin");
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  return `paste-${stamp}.${extension}`;
}

/** Why a file cannot be uploaded at all, before any bytes are sent. */
export function uploadProblem(file: { size: number }): string | undefined {
  if (file.size <= 0) return "This file is empty, so nothing was attached.";
  if (file.size > MAX_ATTACHMENT_BYTES) return "This file is larger than the 50 MiB attachment limit.";
  return undefined;
}

/** HEIC and HEIF upload fine, but neither client accepts them as images. */
export function imageCaveat(name: string): string | undefined {
  return /\.(heic|heif)$/i.test(name) ? "HEIC photos are pasted as a file path; Claude Code and Codex cannot view them as images." : undefined;
}

export const isImage = (name: string, type = "") => type.startsWith("image/") || ["image", "converted"].includes(mediaKind(name));

/** Files carried by a paste or a drop, if any: those win over any text that came along. */
export function filesFrom(data: DataTransfer | null | undefined): File[] {
  if (!data) return [];
  const files = [...(data.files ?? [])];
  if (files.length) return files;
  return [...(data.items ?? [])].filter(item => item.kind === "file").map(item => item.getAsFile()).filter((file): file is File => !!file);
}

/** Upload one file for a session, reporting progress; XHR because fetch cannot report upload progress. */
export function uploadToSession(sessionId: string, file: File, name: string, progress: (fraction: number) => void): { promise: Promise<Attachment>; abort: () => void } {
  const xhr = new XMLHttpRequest();
  const promise = new Promise<Attachment>((resolve, reject) => {
    xhr.open("POST", `/api/sessions/${encodeURIComponent(sessionId)}/uploads?name=${encodeURIComponent(name)}`);
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    xhr.setRequestHeader("X-AgentDock-Client", "web");
    xhr.responseType = "json";
    xhr.upload.onprogress = event => { if (event.lengthComputable) progress(event.loaded / event.total); };
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300 && xhr.response) resolve(xhr.response as Attachment);
      else reject(new Error((xhr.response as { error?: string } | null)?.error ?? `Upload failed (${xhr.status})`));
    };
    xhr.onerror = () => reject(new Error("Upload failed: the connection was lost."));
    xhr.onabort = () => reject(new Error("Upload cancelled."));
    xhr.send(file);
  });
  return { promise, abort: () => xhr.abort() };
}
