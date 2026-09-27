/**
 * In-app confirmation. `window.confirm` is routed through the Tauri dialog
 * plugin, whose `confirm` command this app does not grant, so it throws
 * instead of asking. App renders the pending request with ConfirmDialog.
 */
export interface ConfirmRequest {
  title: string;
  /** The thing being acted on (project, thread, Intern), shown emphasized. */
  subject: string;
  detail?: string;
  confirmLabel: string;
  resolve: (confirmed: boolean) => void;
}

export const confirmState = $state<{ request: ConfirmRequest | null }>({ request: null });

export function askConfirm(options: Omit<ConfirmRequest, 'resolve'>): Promise<boolean> {
  // A newer question replaces an unanswered one, which counts as cancelled.
  confirmState.request?.resolve(false);
  return new Promise(resolve => {
    confirmState.request = { ...options, resolve };
  });
}

export function answerConfirm(confirmed: boolean) {
  const request = confirmState.request;
  confirmState.request = null;
  request?.resolve(confirmed);
}
