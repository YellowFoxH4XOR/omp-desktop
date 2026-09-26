import { api } from './api';
import { modelKey } from './components/conversation/model-utils';
import { parseSlash, TERMINAL_ONLY } from './slash';
import type { SessionModel } from './session.svelte';

/**
 * What a surface (a thread's composer, Pi Intern) can do for Pi's built-in
 * commands. A missing action means the command isn't available there.
 */
export interface SlashHost {
  threadId: string;
  session: SessionModel;
  setModel: (key: string) => Promise<unknown>;
  setEffort: (level: string) => Promise<unknown>;
  reload: () => Promise<unknown>;
  /** Open the model picker (for `/model` with no name). */
  openModelPicker?: () => void;
  /** `/new`: a new thread, or a fresh Intern conversation. */
  startNew?: () => Promise<unknown>;
  rename?: (title: string) => Promise<unknown>;
  setMode?: (mode: 'plan' | 'auto') => Promise<unknown>;
  /** `/login`: open the Pi terminal. */
  login?: () => void;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Pi's built-in terminal commands don't exist over RPC (as a prompt they'd
 * reach the model as text), so πDesk runs them. Returns false for anything
 * else: extension commands, skills and prompt templates go to Pi.
 */
export async function runBuiltin(message: string, host: SlashHost): Promise<boolean> {
  const slash = parseSlash(message);
  if (!slash) return false;
  const { name, args } = slash;
  const { session } = host;
  const say = (text: string, level: 'info' | 'warn' | 'error' = 'info') => session.notify(level, text);
  const unavailable = () => { say(`/${name} isn't available here.`, 'warn'); return true; };
  if (TERMINAL_ONLY.has(name)) {
    say(`/${name} only exists in Pi's own terminal UI. Open the Pi terminal, run pi, then /${name}.`, 'warn');
    return true;
  }
  switch (name) {
    case 'model': {
      if (!args) { host.openModelPicker?.(); return true; }
      const want = args.toLowerCase();
      const models = session.view.models;
      const match = models.find(model => modelKey(model).toLowerCase() === want)
        ?? models.find(model => model.id.toLowerCase() === want)
        ?? models.find(model => model.id.toLowerCase().includes(want) || model.name.toLowerCase().includes(want));
      if (!match) { say(`No model matches “${args}”. Type /model to choose from the list.`, 'warn'); return true; }
      await host.setModel(modelKey(match));
      say(`Switched to ${match.name} (${match.provider}).`);
      return true;
    }
    case 'thinking': {
      const levels = session.view.levels;
      if (!args) { say(`Effort is ${session.view.effort ?? 'the default'}. Choose one of: ${levels.join(', ') || 'none offered by this model'}.`); return true; }
      const level = args.toLowerCase();
      if (!levels.includes(level)) { say(`“${args}” isn't an effort level here. Choose one of: ${levels.join(', ')}.`, 'warn'); return true; }
      await host.setEffort(level);
      say(`Effort set to ${level}.`);
      return true;
    }
    case 'compact':
      say('Compacting the conversation…');
      try {
        await api.compactThread(host.threadId, args || undefined);
        const usage = await api.getUsage(host.threadId).catch(() => null);
        if (usage?.contextUsage) session.view.contextUsage = usage.contextUsage;
        say('Compacted: older context is now a summary.');
      } catch (error) { say(`Could not compact: ${errorText(error)}`, 'error'); }
      return true;
    case 'new':
      if (!host.startNew) return unavailable();
      await host.startNew();
      return true;
    case 'name':
      if (!host.rename) return unavailable();
      if (!args) { say('Give the new name, for example /name Fix login bug.', 'warn'); return true; }
      await host.rename(args.split('\n')[0].slice(0, 120));
      say(`Renamed to “${args.split('\n')[0].slice(0, 120)}”.`);
      return true;
    case 'plan':
    case 'auto':
      if (!host.setMode) { say(`/${name} isn't available here${name === 'auto' ? ': Pi Intern always runs in Plan mode' : ''}.`, 'warn'); return true; }
      await host.setMode(name);
      say(name === 'plan' ? 'Plan mode: read-only until you approve a plan.' : 'Auto mode: full tools.');
      return true;
    case 'session':
      try {
        const usage = await api.getUsage(host.threadId);
        if (usage.contextUsage) session.view.contextUsage = usage.contextUsage;
        const context = usage.contextUsage?.percent != null ? ` · context ${Math.round(usage.contextUsage.percent)}% full` : '';
        say(`${usage.tokens.total.toLocaleString()} tokens (${usage.tokens.input.toLocaleString()} in, ${usage.tokens.output.toLocaleString()} out) · $${usage.cost.toFixed(4)}${context}`);
      } catch (error) { say(`Could not read session stats: ${errorText(error)}`, 'error'); }
      return true;
    case 'copy': {
      const last = [...session.view.items].reverse().find(item => item.kind === 'text' && item.text.trim());
      if (!last || last.kind !== 'text') { say('There is no reply to copy yet.', 'warn'); return true; }
      try { await navigator.clipboard.writeText(last.text); say('Copied the last reply.'); }
      catch { say('Could not reach the clipboard.', 'error'); }
      return true;
    }
    case 'reload':
      await host.reload();
      return true;
    case 'login':
      if (!host.login) return unavailable();
      host.login();
      say('In the Pi terminal, type /login and pick your provider.');
      return true;
    default:
      return false;
  }
}
