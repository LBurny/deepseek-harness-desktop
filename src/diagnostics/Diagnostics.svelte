<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { onMount, onDestroy } from 'svelte'
  import { t } from '../i18n'

  type Status = { state: string; port: number | null; pid: number | null; version: string }
  type Remote = { phase: string; url: string | null; error: string | null }

  let status = $state<Status | null>(null)
  let remote = $state<Remote | null>(null)
  let logs = $state<string[]>([])
  let restarting = $state(false)
  let logError = $state('')
  let logEl: HTMLPreElement | undefined = $state()

  let stateText = $derived(
    !status
      ? '…'
      : status.state.startsWith('Ready')
        ? t('运行中')
        : status.state === 'Starting'
          ? t('启动中')
          : status.state === 'Stopped'
            ? t('已停止')
            : t('失败'),
  )

  let remoteText = $derived(
    !remote
      ? '…'
      : remote.phase === 'up'
        ? (remote.url ?? t('运行中'))
        : remote.phase === 'starting'
          ? t('启动中')
          : remote.phase === 'error'
            ? (remote.error ?? t('失败'))
            : t('未开启'),
  )

  async function refresh() {
    status = await invoke<Status>('get_status')
    remote = await invoke<Remote>('get_remote_status')
  }

  async function restart() {
    restarting = true
    await invoke('restart_dsh')
    setTimeout(() => {
      restarting = false
      refresh()
    }, 1500)
  }

  async function openLog() {
    logError = ''
    try {
      await invoke('open_log_file')
    } catch (e) {
      logError = String(e)
    }
  }

  let unlistenLog: (() => void) | undefined
  let timer = 0

  onMount(async () => {
    await refresh()
    logs = await invoke<string[]>('get_recent_logs')
    unlistenLog = await listen<string>('dsh-log', (e) => {
      logs = [...logs.slice(-499), e.payload]
    })
    timer = window.setInterval(refresh, 2000)
  })

  onDestroy(() => {
    unlistenLog?.()
    clearInterval(timer)
  })

  $effect(() => {
    if (logEl && logs.length > 0) {
      logEl.scrollTop = logEl.scrollHeight
    }
  })
</script>

<main>
  <header>
    <div class="head-left">
      <h1>{t('诊断面板')}</h1>
      <span class="badge" class:ok={stateText === t('运行中')} class:bad={stateText === t('失败')}>{stateText}</span>
    </div>
    <div class="actions">
      <button class="ghost" onclick={openLog}>{t('打开日志')}</button>
      <button class="primary" onclick={restart} disabled={restarting}>
        {restarting ? t('重启中…') : t('重启服务')}
      </button>
    </div>
  </header>

  <section class="card info">
    <div class="row"><span>{t('版本')}</span><b>{status?.version ?? '…'}</b></div>
    <div class="row"><span>{t('端口')}</span><b>{status?.port ?? '—'}</b></div>
    <div class="row"><span>{t('进程 PID')}</span><b>{status?.pid ?? '—'}</b></div>
    <div class="row"><span>{t('远程访问')}</span><b class:bad={remote?.phase === 'error'}>{remoteText}</b></div>
    {#if status && status.state.startsWith('Failed')}
      <div class="row"><span>{t('错误')}</span><b class="bad">{status.state}</b></div>
    {/if}
  </section>

  <section class="card list">
    <h2>{t('诊断日志')}</h2>
    {#if logError}<p class="operr">{logError}</p>{/if}
    <pre class="logs" bind:this={logEl}>{#each logs as line}{line + '\n'}{/each}</pre>
  </section>
</main>

<style>
  main {
    padding: 20px 24px;
    height: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 14px;
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .head-left {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  h1 {
    font-size: 18px;
    margin: 0;
  }
  h2 {
    margin: 0;
    padding: 12px 16px 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
  }
  .badge {
    font-size: 12px;
    padding: 2px 10px;
    border-radius: 10px;
    background: var(--bg-track);
    color: var(--text-2);
  }
  .badge.ok {
    background: rgba(46, 160, 67, 0.18);
    color: var(--ok);
  }
  .badge.bad {
    background: var(--bad-soft-bg);
    color: var(--bad);
  }
  .actions {
    display: flex;
    gap: 10px;
  }
  button {
    border: none;
    border-radius: 8px;
    padding: 8px 18px;
    font-size: 13px;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .primary {
    background: var(--accent);
    color: #fff;
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text-2);
  }
  .card {
    background: var(--bg-raise);
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  .info {
    flex-shrink: 0;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .row {
    display: flex;
    justify-content: space-between;
    font-size: 13px;
    color: var(--text-2);
  }
  .row b {
    color: var(--text);
    font-weight: 500;
  }
  .row b.bad {
    color: var(--bad);
  }
  .list {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .operr {
    margin: 6px 16px 0;
    font-size: 12px;
    color: var(--bad);
  }
  .logs {
    flex: 1;
    min-height: 0;
    margin: 8px 12px 12px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 12px;
    overflow-y: auto;
    font-family: 'Cascadia Mono', Consolas, monospace;
    font-size: 12px;
    line-height: 1.55;
    color: var(--text-2);
    white-space: pre-wrap;
    word-break: break-all;
  }
</style>