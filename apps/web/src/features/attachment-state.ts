/**
 * Attachments are stored inside the workspace and referenced by path.
 *
 * Both native clients can already read workspace files with their own tools, so
 * a path works for every provider and every file type. Nothing here converts a
 * file into inline model content or claims a client can see an image it was
 * never given.
 */
export interface Attachment {
  path: string;
  name: string;
  bytes: number;
}
/** Matches the server's own cap, so an oversized pick fails before uploading. */
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;
export const MAX_ATTACHMENTS_PER_MESSAGE = 10;

export function attachmentError(file: { name: string; size: number }, existing: number): string | undefined {
  if (existing >= MAX_ATTACHMENTS_PER_MESSAGE) return 'Attach at most {count} files to one message.';
  if (file.size <= 0) return 'This file is empty, so nothing was attached.';
  if (file.size > MAX_ATTACHMENT_BYTES) return 'This file is larger than the 10 MiB attachment limit.';
  return undefined;
}

/**
 * Message text sent to the agent. Paths are listed above the user's own words so
 * the agent sees what was attached, and the list is omitted entirely when there
 * is nothing to attach — an empty header would just be noise in the transcript.
 */
export function composeMessage(text: string, attachments: Attachment[]): string {
  const body = text.trim();
  if (!attachments.length) return body;
  const lines = attachments.map(attachment => `- ${attachment.path}`).join('\n');
  const header = `Attached ${attachments.length === 1 ? 'file' : 'files'} in this workspace:\n${lines}`;
  return body ? `${header}\n\n${body}` : header;
}

/** A message can be sent with attachments alone, with no typed text. */
export function canSendMessage(text: string, attachments: Attachment[]): boolean {
  return text.trim().length > 0 || attachments.length > 0;
}

export function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value < 0) return '0 B';
  if (value < 1024) return `${Math.round(value)} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(value < 10 * 1024 ? 1 : 0).replace(/\.0$/, '')} KB`;
  return `${(value / (1024 * 1024)).toFixed(value < 10 * 1024 * 1024 ? 1 : 0).replace(/\.0$/, '')} MB`;
}
