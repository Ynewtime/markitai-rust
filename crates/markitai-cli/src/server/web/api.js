// Token handling is confined to this service; provider credentials never enter storage.
let token = '';
const TOKEN_KEY = 'markitai.service-token';
export function serviceURL(path, origin = location.origin) {
  const url = new URL(path, origin);
  if (url.origin !== origin || !['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error('External service URL rejected');
  return url;
}
export function bootstrapToken(loc = location, hist = history, storage) {
  const url = new URL(loc.href);
  const fragment = new URLSearchParams(url.hash.slice(1));
  const supplied = fragment.get('token') ?? url.searchParams.get('token');
  if (supplied !== null) {
    token = supplied;
    fragment.delete('token'); url.searchParams.delete('token');
    url.hash = fragment.toString();
    hist.replaceState(null, '', url.pathname + url.search + url.hash);
    try { (storage ?? globalThis.sessionStorage).setItem(TOKEN_KEY, token); } catch { /* Memory remains usable when storage is unavailable. */ }
  } else {
    try { token = (storage ?? globalThis.sessionStorage).getItem(TOKEN_KEY) || ''; } catch { token = ''; }
  }
}
export function setToken(value) {
  token = value.trim();
  try { sessionStorage.setItem(TOKEN_KEY, token); } catch { /* Memory-only token. */ }
}
export function authenticatedURL(path) {
  const url = serviceURL(path);
  if (!url.pathname.startsWith('/api/')) throw new Error('Tokens are restricted to service API URLs');
  if (token) url.searchParams.set('token', token);
  return url.href;
}
export class ApiError extends Error {
  constructor(status, body) {
    const detail = body?.detail;
    super(typeof detail === 'string' ? detail : detail?.code || `Request failed (${status})`);
    this.status = status; this.body = body;
  }
}
export async function api(path, {method = 'GET', body, signal, text = false} = {}) {
  const url = serviceURL(path);
  const headers = new Headers();
  if (token) headers.set('Authorization', `Bearer ${token}`);
  if (body !== undefined && !(body instanceof FormData)) {
    headers.set('Content-Type', 'application/json'); body = JSON.stringify(body);
  }
  const response = await fetch(url, {method, body, headers, signal, credentials: 'same-origin', redirect: 'error', cache: 'no-store'});
  if (!response.ok) {
    let value; try { value = await response.json(); } catch { value = null; }
    const error = new ApiError(response.status, value);
    if (response.status === 401) window.dispatchEvent(new CustomEvent('markitai:unauthorized'));
    throw error;
  }
  return response.status === 204 ? null : text ? response.text() : response.json();
}
export function fileURL(job, path) {
  return `/api/jobs/${encodeURIComponent(job)}/files/${path.split('/').map(encodeURIComponent).join('/')}`;
}
export function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}
export function button(text, action, className = 'quiet small') {
  const node = element('button', className, text); node.type = 'button';
  node.addEventListener('click', action); return node;
}
export function errorText(error) { return error instanceof Error ? error.message : 'Request failed'; }
export function artifactPath(target, documentPath, allowed) {
  if (/^[a-z][a-z\d+.-]*:/i.test(target) || target.startsWith('//') || target.startsWith('/') || target.includes('\\')) return null;
  let url;
  try { url = new URL(target, 'https://artifact.invalid/' + documentPath.split('/').map(encodeURIComponent).join('/')); } catch { return null; }
  let path;
  try { path = decodeURIComponent(url.pathname.slice(1)); } catch { return null; }
  return allowed.has(path) ? path : null;
}
