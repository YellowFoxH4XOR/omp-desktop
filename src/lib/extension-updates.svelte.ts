import { api } from './api';
import type { InstalledPackage } from './types';

/** How long a finished update check is reused before checking npm again. */
export const RECHECK_MS = 15 * 60 * 1000;

/**
 * The last npm update check, shared by the startup check (Settings badge)
 * and the Extensions page, so reopening the page shows the same results.
 */
export const updateCheck = $state<{ at: number | null; packages: InstalledPackage[]; running: boolean }>({
  at: null,
  packages: [],
  running: false,
});

export function pendingUpdates(): number {
  return updateCheck.packages.filter(item => item.updateAvailable).length;
}

export function isFresh(): boolean {
  return updateCheck.at !== null && Date.now() - updateCheck.at < RECHECK_MS;
}

/** npm packages the last check could not get a version for. */
export function uncheckedCount(packages: InstalledPackage[]): number {
  return updateCheck.at === null ? 0 : packages.filter(item => item.kind === 'npm' && !item.latest).length;
}

export async function checkForUpdates(): Promise<InstalledPackage[]> {
  updateCheck.running = true;
  try {
    const packages = await api.extensionsCheckUpdates();
    updateCheck.packages = packages;
    updateCheck.at = Date.now();
    return packages;
  } finally {
    updateCheck.running = false;
  }
}
