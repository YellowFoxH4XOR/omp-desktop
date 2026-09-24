import { openUrl } from '@tauri-apps/plugin-opener';
import { isTauri } from '@tauri-apps/api/core';

const SAFE_PROTOCOLS = new Set(['http:', 'https:', 'mailto:']);

/**
 * Open an external link with the system browser via the opener plugin.
 * Never navigates the webview itself and refuses non-web schemes.
 */
export async function openExternal(url: string): Promise<boolean> {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  if (!SAFE_PROTOCOLS.has(parsed.protocol)) return false;
  try {
    await openUrl(parsed.toString());
    return true;
  } catch {
    // A denied native open must not be bypassed via window.open.
    if (isTauri()) return false;
    return window.open(parsed.toString(), '_blank', 'noopener,noreferrer') !== null;
  }
}

/**
 * Delegated click handler for sanitized markdown containers. Intercepts
 * anchor clicks and routes them through the opener plugin.
 */
export function handleMarkdownClick(event: MouseEvent): void {
  const anchor = (event.target as HTMLElement | null)?.closest?.('a');
  if (!anchor) return;
  const href = anchor.getAttribute('href');
  if (!href) return;
  event.preventDefault();
  void openExternal(href);
}
