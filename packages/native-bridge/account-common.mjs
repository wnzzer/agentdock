// What every client's account adapter shares: display sanitizing, the empty
// capability set, and the error a client's own refusal becomes.
export const clean = (value, length = 240) => typeof value === 'string' ? value.replace(/[\u0000-\u001f\u007f]/g, ' ').slice(0, length) : undefined;
export const numeric = value => typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined;
export const noCapabilities = () => ({ login: false, refresh_token: false, quota: false, reset_quota: false, logout: false });

export class RpcError extends Error {
  constructor(code) { super(code === -32601 ? 'Installed client does not support this official account operation.' : 'Official account operation failed.'); this.code = code; }
}
