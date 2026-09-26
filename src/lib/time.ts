/** "just now", "5 minutes ago", "3 days ago". */
export function ago(iso: string, now = Date.now()): string {
  const seconds = Math.max(0, (now - Date.parse(iso)) / 1000);
  if (!Number.isFinite(seconds)) return '';
  if (seconds < 60) return 'just now';
  const [value, unit] = seconds < 3600 ? [seconds / 60, 'minute'] : seconds < 86400 ? [seconds / 3600, 'hour'] : seconds < 2592000 ? [seconds / 86400, 'day'] : [seconds / 2592000, 'month'];
  const count = Math.floor(value);
  return `${count} ${unit}${count === 1 ? '' : 's'} ago`;
}

/** Elapsed time as m:ss, or h:mm:ss past an hour. */
export function clock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600), m = Math.floor((total % 3600) / 60), s = total % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}
