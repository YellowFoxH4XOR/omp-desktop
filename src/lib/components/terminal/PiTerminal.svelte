<script lang="ts">
  import { onMount } from 'svelte';
  import { Channel } from '@tauri-apps/api/core';
  import { api } from '$lib/api';
  import { openExternal } from '$lib/components/conversation/links';
  import { isDarkTheme } from '$lib/components/conversation/mermaid';

  /** Acknowledge rendered output in batches; the backend pauses the shell
   *  once 1 MiB is unacknowledged, so a flood can never outrun the renderer. */
  const ACK_BATCH_BYTES = 64 * 1024;
  const ACK_FLUSH_MS = 50;
  /** Readable ANSI colours on the light background (xterm's defaults assume dark). */
  const LIGHT_ANSI = {
    black: '#1c1c1e', red: '#c62828', green: '#1c7a4c', yellow: '#946200',
    blue: '#2f68e8', magenta: '#8250df', cyan: '#0e7490', white: '#5d5d65',
    brightBlack: '#6e6e76', brightRed: '#d63a3a', brightGreen: '#228a5a', brightYellow: '#a4670b',
    brightBlue: '#3b73f0', brightMagenta: '#8c5ce0', brightCyan: '#127f9e', brightWhite: '#1c1c1e',
  };

  let { cwd }: { cwd?: string } = $props();
  let host: HTMLDivElement;
  let error = $state('');

  onMount(() => {
    let disposed = false;
    let id: string | undefined;
    let observer: ResizeObserver | undefined;
    let themeObserver: MutationObserver | undefined;
    let cleanup: (() => void) | undefined;
    void (async () => {
      const [{ Terminal }, { FitAddon }, { WebLinksAddon }] = await Promise.all([
        import('@xterm/xterm'), import('@xterm/addon-fit'), import('@xterm/addon-web-links'),
        import('@xterm/xterm/css/xterm.css'),
      ]);
      if (disposed) return;
      const term = new Terminal({ cursorBlink: true, fontFamily: 'SF Mono, Menlo, monospace', fontSize: 12, allowProposedApi: false });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.loadAddon(new WebLinksAddon((_event, url) => { void openExternal(url); }));
      term.open(host);
      const applyTheme = () => {
        const css = getComputedStyle(document.documentElement);
        term.options.theme = {
          background: css.getPropertyValue('--bg').trim(),
          foreground: css.getPropertyValue('--text').trim(),
          cursor: css.getPropertyValue('--accent').trim(),
          selectionBackground: css.getPropertyValue('--accent-bg').trim(),
          ...(isDarkTheme() ? {} : LIGHT_ANSI),
        };
      };
      applyTheme();
      themeObserver = new MutationObserver(applyTheme);
      themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
      const preference = matchMedia('(prefers-color-scheme: light)');
      preference.addEventListener('change', applyTheme);
      // Output can arrive before terminalOpen resolves; acks wait for the id.
      let pendingAck = 0;
      let ackTimer: ReturnType<typeof setTimeout> | undefined;
      const flushAck = () => {
        clearTimeout(ackTimer);
        ackTimer = undefined;
        if (!id || disposed || pendingAck === 0) return;
        const bytes = pendingAck;
        pendingAck = 0;
        void api.terminalAck(id, bytes).catch(() => undefined);
      };
      const output = new Channel<ArrayBuffer>();
      output.onmessage = data => {
        if (disposed) return;
        const bytes = new Uint8Array(data);
        term.write(bytes, () => {
          pendingAck += bytes.byteLength;
          if (pendingAck >= ACK_BATCH_BYTES) flushAck();
          else ackTimer ??= setTimeout(flushAck, ACK_FLUSH_MS);
        });
      };
      const fitTerminal = () => {
        if (disposed || !host.isConnected) return;
        fit.fit();
        if (id && term.cols >= 2 && term.rows >= 2) void api.terminalResize(id, term.cols, term.rows).catch(() => undefined);
      };
      observer = new ResizeObserver(fitTerminal);
      observer.observe(host);
      fitTerminal();
      cleanup = () => { clearTimeout(ackTimer); preference.removeEventListener('change', applyTheme); term.dispose(); };
      const opened = await api.terminalOpen(Math.max(2, term.cols), Math.max(2, term.rows), cwd, output);
      if (disposed) { void api.terminalClose(opened); return; }
      id = opened;
      flushAck();
      // Chain writes so keystrokes reach the backend queue in typing order.
      let writeChain: Promise<unknown> = Promise.resolve();
      term.onData(data => {
        const target = id;
        if (!target) return;
        writeChain = writeChain.then(() => api.terminalWrite(target, data)).catch(() => undefined);
      });
      fitTerminal();
      term.focus();
    })().catch(reason => { if (!disposed) error = `Could not open terminal: ${reason instanceof Error ? reason.message : String(reason)}`; });
    return () => {
      disposed = true;
      observer?.disconnect();
      themeObserver?.disconnect();
      cleanup?.();
      if (id) void api.terminalClose(id);
    };
  });
</script>

<div class="terminal-wrap">
  {#if error}<div class="terminal-error" role="alert">{error}</div>{/if}
  <div class="terminal-host" bind:this={host} role="application" aria-label="Private Pi terminal"></div>
</div>

<style>
  .terminal-wrap { flex:1; display:flex; flex-direction:column; min-height:0; min-width:0; background:var(--bg); }
  .terminal-host { flex:1; min-height:0; padding:12px; }
  .terminal-error { padding:10px 14px; color:var(--bad); font-size:12px; }
  .terminal-host :global(.xterm) { height:100%; }
</style>
