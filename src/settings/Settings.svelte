<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { getVersion } from '@tauri-apps/api/app'
  import { onMount } from 'svelte'
  import { t } from '../i18n'
  import PopupSelect from './PopupSelect.svelte'

  type Shortcut = { ctrl: boolean; shift: boolean; alt: boolean; code: string; key: string }
  type CompletionSound =
    | 'silent'
    | 'default'
    | 'bip-bop-01' | 'bip-bop-02' | 'bip-bop-03' | 'bip-bop-04' | 'bip-bop-05'
    | 'bip-bop-06' | 'bip-bop-07' | 'bip-bop-08' | 'bip-bop-09' | 'bip-bop-10'
    | 'staplebops-01' | 'staplebops-02' | 'staplebops-03' | 'staplebops-04'
    | 'staplebops-05' | 'staplebops-06' | 'staplebops-07'
  type NotifyTiming = 'background' | 'always'
  type NotifyRule = { enabled: boolean; timing: NotifyTiming }
  // 与 Rust 端 NotifySettings 对应：approval=任务确认 question=选项选择
  // turn_done=任务完成（干活回合） answer_done=回答完成（纯文字回答）
  type NotifySettings = { approval: NotifyRule; question: NotifyRule; turn_done: NotifyRule; answer_done: NotifyRule }
  type ShellSettings = {
    zoom_step: number
    zoom_in: Shortcut
    zoom_out: Shortcut
    close_behavior: 'background' | 'quit'
    notify: NotifySettings
    completion_sound: CompletionSound
    check_update_on_launch: boolean
  }
  // 与 Rust 端 update::UpdateInfo 对应
  type UpdateInfo = {
    current: string
    latest: string
    has_update: boolean
    release_url: string
    asset_size: number | null
  }

  // 与 Rust 端 ShellSettings::default 保持一致（四类通知默认均开、仅后台时提醒）
  const DEFAULT_RULE: NotifyRule = { enabled: true, timing: 'background' }
  const DEFAULTS: ShellSettings = {
    zoom_step: 0.02,
    zoom_in: { ctrl: true, shift: true, alt: false, code: 'Equal', key: '+' },
    zoom_out: { ctrl: true, shift: true, alt: false, code: 'Minus', key: '_' },
    close_behavior: 'background',
    notify: { approval: { ...DEFAULT_RULE }, question: { ...DEFAULT_RULE }, turn_done: { ...DEFAULT_RULE }, answer_done: { ...DEFAULT_RULE } },
    completion_sound: 'staplebops-02',
    check_update_on_launch: false,
  }

  // label 存中文原文，模板里经 t() 渲染——locale 切换时选项文字同步更新
  const SOUND_OPTIONS: { value: CompletionSound; label: string }[] = [
    { value: 'silent', label: '无提示音' },
    { value: 'default', label: '系统默认' },
    { value: 'bip-bop-01', label: '哔啵 01' },
    { value: 'bip-bop-02', label: '哔啵 02' },
    { value: 'bip-bop-03', label: '哔啵 03' },
    { value: 'bip-bop-04', label: '哔啵 04' },
    { value: 'bip-bop-05', label: '哔啵 05' },
    { value: 'bip-bop-06', label: '哔啵 06' },
    { value: 'bip-bop-07', label: '哔啵 07' },
    { value: 'bip-bop-08', label: '哔啵 08' },
    { value: 'bip-bop-09', label: '哔啵 09' },
    { value: 'bip-bop-10', label: '哔啵 10' },
    { value: 'staplebops-01', label: '叮当 01' },
    { value: 'staplebops-02', label: '叮当 02' },
    { value: 'staplebops-03', label: '叮当 03' },
    { value: 'staplebops-04', label: '叮当 04' },
    { value: 'staplebops-05', label: '叮当 05' },
    { value: 'staplebops-06', label: '叮当 06' },
    { value: 'staplebops-07', label: '叮当 07' },
  ]

  // label 存中文原文，模板里经 t() 渲染——locale 切换时选项文字同步更新
  const TIMING_OPTIONS: { value: NotifyTiming; label: string }[] = [
    { value: 'background', label: '仅后台时提醒' },
    { value: 'always', label: '总是提醒' },
  ]

  // label 存中文原文，模板里经 t() 渲染——locale 切换时选项文字同步更新
  const NOTIFY_ROWS: { key: keyof NotifySettings; label: string }[] = [
    { key: 'approval', label: '任务确认' },
    { key: 'question', label: '选项选择' },
    { key: 'turn_done', label: '任务完成' },
    { key: 'answer_done', label: '回答完成' },
  ]

  let zoomIn = $state<Shortcut>({ ...DEFAULTS.zoom_in })
  let zoomOut = $state<Shortcut>({ ...DEFAULTS.zoom_out })
  let stepPct = $state(2)
  let closeBehavior = $state<'background' | 'quit'>('background')
  let notify = $state<NotifySettings>(structuredClone(DEFAULTS.notify))
  // 提示音作用于全部通知（任务确认/选项选择/任务完成/回答完成统一用这一个音效）；
  // 四类全关时才禁用音效选择与试听
  const anyNotifyEnabled = $derived(
    notify.approval.enabled ||
      notify.question.enabled ||
      notify.turn_done.enabled ||
      notify.answer_done.enabled,
  )
  let completionSound = $state<CompletionSound>('default')
  let autostart = $state(false)
  let checkOnLaunch = $state(false)
  let recording = $state<'in' | 'out' | null>(null)
  let saving = $state(false)
  let notice = $state<{ kind: 'ok' | 'err'; text: string } | null>(null)

  // 检查更新区块：idle → checking → latest / downloading → done / error
  type UpdatePhase = 'idle' | 'checking' | 'latest' | 'downloading' | 'done' | 'error'
  let appVersion = $state('')
  let updatePhase = $state<UpdatePhase>('idle')
  let updateVersion = $state('')
  let updateName = $state('')
  let updatePath = $state('')
  let updateErr = $state('')
  let dlDownloaded = $state(0)
  let dlTotal = $state(0)

  onMount(() => {
    let unlisten: (() => void) | undefined
    ;(async () => {
      const [s, auto] = await Promise.all([
        invoke<ShellSettings>('get_shell_settings'),
        invoke<boolean>('get_autostart'),
      ])
      applyState(s)
      autostart = auto
      // 下载进度由后端 download_update 按百分比变化节流推送
      unlisten = await listen<{ downloaded: number; total: number }>(
        'update-download-progress',
        (e) => {
          if (updatePhase !== 'downloading') return
          dlDownloaded = e.payload.downloaded
          dlTotal = e.payload.total
        },
      )
    })()
    getVersion()
      .then((v) => (appVersion = v))
      .catch(() => {})
    return () => unlisten?.()
  })

  function applyState(s: ShellSettings) {
    zoomIn = { ...s.zoom_in }
    zoomOut = { ...s.zoom_out }
    stepPct = Math.round(s.zoom_step * 100)
    closeBehavior = s.close_behavior
    notify = structuredClone(s.notify)
    completionSound = s.completion_sound
    checkOnLaunch = s.check_update_on_launch
  }

  const CODE_LABELS: Record<string, string> = {
    Equal: '=', Minus: '-', Comma: ',', Period: '.', Slash: '/', Backslash: '\\',
    Semicolon: ';', Quote: "'", BracketLeft: '[', BracketRight: ']', Backquote: '`',
    Space: 'Space', ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→',
  }

  function shortcutLabel(sc: Shortcut): string {
    const base =
      CODE_LABELS[sc.code] ??
      (sc.code.startsWith('Key')
        ? sc.code.slice(3)
        : sc.code.startsWith('Digit')
          ? sc.code.slice(5)
          : sc.code || sc.key)
    return [sc.ctrl && 'Ctrl', sc.shift && 'Shift', sc.alt && 'Alt', base]
      .filter(Boolean)
      .join('+')
  }

  // 录制快捷键：捕获首个非修饰键的 keydown；Esc 取消。监听器自摘除。
  function record(which: 'in' | 'out') {
    if (recording) return
    recording = which
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault()
      e.stopPropagation()
      if (['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) return
      window.removeEventListener('keydown', onKey, true)
      recording = null
      if (e.key === 'Escape') return
      const sc: Shortcut = { ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey, code: e.code, key: e.key }
      if (which === 'in') zoomIn = sc
      else zoomOut = sc
    }
    window.addEventListener('keydown', onKey, true)
  }

  function sameShortcut(a: Shortcut, b: Shortcut): boolean {
    return (
      a.ctrl === b.ctrl && a.shift === b.shift && a.alt === b.alt &&
      a.code === b.code && a.key === b.key
    )
  }

  function clientValidate(): string | null {
    for (const [name, sc] of [[t('放大'), zoomIn], [t('缩小'), zoomOut]] as const) {
      if (!(sc.ctrl || sc.shift || sc.alt)) {
        return `${name}${t('快捷键必须包含 Ctrl/Shift/Alt 中至少一个修饰键')}`
      }
    }
    if (sameShortcut(zoomIn, zoomOut)) return t('放大与缩小快捷键不能相同')
    return null
  }

  async function save() {
    notice = null
    const err = clientValidate()
    if (err) {
      notice = { kind: 'err', text: err }
      return
    }
    saving = true
    try {
      const next: ShellSettings = {
        zoom_step: Math.min(Math.max(stepPct, 1), 25) / 100,
        zoom_in: zoomIn,
        zoom_out: zoomOut,
        close_behavior: closeBehavior,
        notify,
        completion_sound: completionSound,
        check_update_on_launch: checkOnLaunch,
      }
      await invoke('set_shell_settings', { next })
      await invoke('set_autostart', { enabled: autostart })
      notice = { kind: 'ok', text: t('已保存，立即生效') }
    } catch (e) {
      notice = { kind: 'err', text: String(e) }
    } finally {
      saving = false
    }
  }

  function resetDefaults() {
    applyState(structuredClone(DEFAULTS))
    autostart = false
    notice = { kind: 'ok', text: t('已恢复默认值，点击保存生效') }
  }

  // 试听：toast 的音效是它的属性，只能连同通知一起听
  async function previewSound() {
    try {
      await invoke('preview_completion_sound', { sound: completionSound })
    } catch (e) {
      notice = { kind: 'err', text: String(e) }
    }
  }

  // 手动更新：检查到新版后直接下载安装包（不跳转浏览器），
  // 进度条由 update-download-progress 事件驱动；完成后给出"立即安装"
  async function manualUpdate() {
    if (updatePhase === 'checking' || updatePhase === 'downloading') return
    updatePhase = 'checking'
    try {
      const info = await invoke<UpdateInfo>('check_update')
      appVersion = info.current
      if (!info.has_update) {
        updatePhase = 'latest'
        return
      }
      updateVersion = info.latest
      dlDownloaded = 0
      dlTotal = info.asset_size ?? 0
      updatePhase = 'downloading'
      const path = await invoke<string>('download_update')
      updatePath = path
      updateName = path.split(/[\\/]/).pop() ?? path
      updatePhase = 'done'
    } catch (e) {
      updateErr = String(e)
      updatePhase = 'error'
    }
  }

  async function openUpdatePage() {
    try {
      await invoke('open_update_page')
    } catch (e) {
      updateErr = String(e)
      updatePhase = 'error'
    }
  }

  // 启动已下载的 NSIS 安装包；成功后壳会自行退出（安装器是壳的子进程，
  // 壳不死则安装器钩子里的 taskkill 会连它一起杀掉），用户随后在安装向导里完成覆盖安装
  async function installDownloaded() {
    try {
      await invoke('install_update', { path: updatePath })
    } catch (e) {
      updateErr = String(e)
      updatePhase = 'error'
    }
  }
</script>

<main>
  <header>
    <h1>{t('其它设置')}</h1>
  </header>

  <section class="card">
    <h2>{t('通用')}</h2>
    <label class="check">
      <input type="checkbox" bind:checked={autostart} />
      {t('开机时自动启动')}
    </label>
    <div class="divider"></div>
    <span class="group-label">{t('关闭主窗口时')}</span>
    <label class="check">
      <input type="radio" bind:group={closeBehavior} value="background" />
      {t('最小化到托盘')}
    </label>
    <label class="check">
      <input type="radio" bind:group={closeBehavior} value="quit" />
      {t('退出程序')}
    </label>
  </section>

  <section class="card">
    <h2>{t('检查更新')}</h2>
    <label class="check">
      <input type="checkbox" bind:checked={checkOnLaunch} />
      {t('启动时自动检查更新')}
    </label>
    <div class="divider"></div>
    <div class="row">
      <span>{t('当前版本')} v{appVersion}</span>
      <span class="control">
        <button class="ghost small" onclick={openUpdatePage}>{t('GitHub 下载')}</button>
        <button
          class="ghost small"
          onclick={manualUpdate}
          disabled={updatePhase === 'checking' || updatePhase === 'downloading'}
        >
          {updatePhase === 'checking' ? t('正在检查更新…') : t('手动更新')}
        </button>
      </span>
    </div>
    {#if updatePhase !== 'idle'}
      <div class="update-status">
        {#if updatePhase === 'checking'}
          <span class="status-text">{t('正在检查更新…')}</span>
        {:else if updatePhase === 'latest'}
          <span class="status-text ok">{t('当前已是最新版本')}</span>
        {:else if updatePhase === 'downloading'}
          {@const pct = dlTotal > 0 ? Math.min(Math.round((dlDownloaded / dlTotal) * 100), 100) : 0}
          <span class="status-text">
            {t('发现新版本 {ver}，正在下载…', { ver: updateVersion })}{dlTotal > 0 ? ` ${pct}%` : ''}
          </span>
          <div class="track"><div class="bar" style="width: {pct}%"></div></div>
        {:else if updatePhase === 'done'}
          <span class="status-text ok">{t('已下载：{name}', { name: updateName })}</span>
          <button class="ghost small" onclick={installDownloaded}>{t('立即安装')}</button>
        {:else if updatePhase === 'error'}
          <span class="status-text err">{updateErr}</span>
        {/if}
      </div>
    {/if}
  </section>

  <section class="card">
    <h2>{t('通知提醒')}</h2>
    {#each NOTIFY_ROWS as row (row.key)}
      {@const rule = notify[row.key]}
      <div class="row">
        <label class="check">
          <input type="checkbox" bind:checked={rule.enabled} />
          {t(row.label)}
        </label>
        <span class="control">
          <PopupSelect bind:value={rule.timing} options={TIMING_OPTIONS} disabled={!rule.enabled} />
        </span>
      </div>
    {/each}
    <div class="divider"></div>
    <div class="row">
      <span>{t('提示音')}</span>
      <span class="control">
        <PopupSelect
          bind:value={completionSound}
          options={SOUND_OPTIONS}
          disabled={!anyNotifyEnabled}
        />
        <button
          class="ghost small"
          onclick={previewSound}
          disabled={!anyNotifyEnabled || completionSound === 'silent'}
        >
          {t('试听')}
        </button>
      </span>
    </div>
  </section>

  <section class="card">
    <h2>{t('界面缩放')}</h2>
    <div class="row">
      <span>{t('每次放大/缩小比例')}</span>
      <span class="control">
        <input type="number" min="1" max="25" bind:value={stepPct} /> %
      </span>
    </div>
    <div class="row">
      <span>{t('放大快捷键')}</span>
      <button class="recorder" class:recording={recording === 'in'} onclick={() => record('in')}>
        {recording === 'in' ? t('按下快捷键…（Esc 取消）') : shortcutLabel(zoomIn)}
      </button>
    </div>
    <div class="row">
      <span>{t('缩小快捷键')}</span>
      <button class="recorder" class:recording={recording === 'out'} onclick={() => record('out')}>
        {recording === 'out' ? t('按下快捷键…（Esc 取消）') : shortcutLabel(zoomOut)}
      </button>
    </div>
  </section>

  <section class="actions">
    <button class="primary" onclick={save} disabled={saving}>{saving ? t('保存中…') : t('保存')}</button>
    <button class="ghost" onclick={resetDefaults}>{t('恢复默认')}</button>
    {#if notice}
      <p class="notice" class:err={notice.kind === 'err'}>{notice.text}</p>
    {/if}
  </section>
</main>

<style>
  main {
    padding: 20px 24px;
    height: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 16px;
    overflow-y: auto;
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  h1 {
    font-size: 18px;
    margin: 0;
  }
  h2 {
    font-size: 12px;
    letter-spacing: 0.08em;
    color: var(--text-2);
    margin: 0;
    font-weight: 600;
  }
  .card {
    background: var(--bg-raise);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    min-height: 36px;
    font-size: 13px;
    color: var(--text-2);
  }
  .control {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text);
  }
  input[type='number'] {
    width: 64px;
    height: 32px;
    box-sizing: border-box;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    padding: 6px 8px;
    font-size: 13px;
  }
  .ghost.small {
    height: 32px;
    box-sizing: border-box;
    padding: 0 12px;
    font-size: 12px;
    border-radius: 6px;
  }
  .ghost.small:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .update-status {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .status-text {
    font-size: 12px;
    color: var(--text-2);
  }
  .status-text.ok {
    color: var(--ok);
  }
  .status-text.err {
    color: var(--bad);
  }
  /* 与启动画面同款的细进度条：border 底色 + accent 填充 */
  .track {
    flex: 1 1 100%;
    height: 4px;
    border-radius: 2px;
    background: var(--border);
    overflow: hidden;
  }
  .bar {
    height: 100%;
    border-radius: 2px;
    background: var(--accent);
    transition: width 0.25s ease-out;
  }
  .recorder {
    min-width: 170px;
    height: 32px;
    box-sizing: border-box;
    text-align: center;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    padding: 6px 12px;
    font-size: 13px;
    cursor: pointer;
  }
  .recorder.recording {
    border-color: var(--accent);
    color: var(--accent-soft);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-2);
    cursor: pointer;
  }
  .divider {
    height: 1px;
    background: var(--border);
  }
  .group-label {
    font-size: 13px;
    color: var(--text-2);
  }
  .notice {
    margin: 0;
    font-size: 12px;
    color: var(--ok);
  }
  .notice.err {
    color: var(--bad);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 12px;
    border-top: 1px solid var(--border);
    padding-top: 16px;
  }
  .actions button {
    border: none;
    border-radius: 8px;
    padding: 8px 18px;
    font-size: 13px;
    cursor: pointer;
  }
  .actions button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .primary {
    background: var(--accent);
    color: #fff;
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--border) !important;
    color: var(--text-2);
  }
</style>

