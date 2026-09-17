/**
 * A unique id that exists outside a secure context.
 *
 * `crypto.randomUUID` is only defined on a secure origin — HTTPS, or localhost.
 * A workspace reached at `http://192.168.0.252:28789` is neither, so there the
 * function is simply missing, and calling it throws. That is not a degraded
 * feature but a broken one: the composer builds a message id before sending, so
 * the throw came out as a message that would not send, on a page where
 * everything else worked.
 *
 * `crypto.getRandomValues` is available in every context, secure or not, so the
 * fallback is a version 4 UUID built from it rather than a different shape of
 * id. The server accepts one format; a fallback that produced another would
 * trade this failure for a later one.
 */
export function randomId(): string {
  const source = globalThis.crypto;
  if (typeof source?.randomUUID === "function") return source.randomUUID();
  const bytes = new Uint8Array(16);
  if (typeof source?.getRandomValues === "function") source.getRandomValues(bytes);
  // Last resort for an environment with no crypto at all. A message id is a
  // de-duplication key rather than a secret, so being merely unique is enough
  // -- and failing to send is worse than a weaker source of randomness.
  else for (let index = 0; index < bytes.length; index++) bytes[index] = Math.floor(Math.random() * 256);
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // Version 4.
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant.
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex.slice(6, 8).join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10, 16).join("")}`;
}
