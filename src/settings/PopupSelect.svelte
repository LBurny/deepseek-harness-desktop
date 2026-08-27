<script lang="ts" generics="T extends string">
  // 本地页面共用的弹层下拉：触发框观感对齐原生 select（边框/底色/32px 高），
  // 弹层与触发框同宽、限高滚动（原生 select 弹层全量铺开且样式不可控，故自绘）。
  // 高亮/选中为原生风格的浅色直角色块；label 存中文原文，经 t() 渲染。
  import { t } from '../i18n'

  type Option = { value: T; label: string }

  let {
    value = $bindable(),
    options,
    disabled = false,
  }: { value: T; options: Option[]; disabled?: boolean } = $props()

  let open = $state(false)
  let rootEl: HTMLDivElement | undefined

  const currentLabel = $derived(options.find((o) => o.value === value)?.label ?? '')

  // 打开时把当前选中项滚进可视区
  function scrollSelectedIntoView(node: HTMLElement) {
    node.querySelector('.pop-item.selected')?.scrollIntoView({ block: 'nearest' })
  }
</script>

<svelte:window
  onpointerdown={(e) => {
    if (open && rootEl && e.target instanceof Node && !rootEl.contains(e.target)) open = false
  }}
  onkeydown={(e) => {
    if (e.key === 'Escape') open = false
  }}
/>

<div class="select" bind:this={rootEl}>
  <button
    type="button"
    class="trigger"
    disabled={disabled}
    onclick={() => (open = !open)}
  >
    <span class="trigger-label">{t(currentLabel)}</span>
    <svg class="caret" viewBox="0 0 10 10" aria-hidden="true">
      <path d="M2 3.5 5 6.5 8 3.5" />
    </svg>
  </button>
  {#if open}
    <div class="pop" role="listbox" use:scrollSelectedIntoView>
      {#each options as opt (opt.value)}
        <button
          type="button"
          class="pop-item"
          class:selected={value === opt.value}
          onclick={() => {
            value = opt.value
            open = false
          }}
        >
          {t(opt.label)}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .select {
    position: relative;
  }
  .trigger {
    width: 112px;
    height: 32px;
    box-sizing: border-box;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 0 8px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text);
    font-size: 13px;
    cursor: pointer;
  }
  .trigger:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .trigger-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .caret {
    flex: none;
    width: 10px;
    height: 10px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.3;
    stroke-linecap: round;
    stroke-linejoin: round;
    color: var(--text-2);
  }
  .pop {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    max-height: 192px; /* opencode listbox 上限 12rem */
    overflow-y: auto;
    overflow-x: hidden;
    z-index: 20;
    box-sizing: border-box;
    background: var(--bg-raise);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
    padding: 4px;
    display: flex;
    flex-direction: column;
    transform-origin: top center;
    animation: pop-in 120ms ease-out;
  }
  @keyframes pop-in {
    from {
      opacity: 0;
      transform: translateY(-2px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
  .pop::-webkit-scrollbar {
    width: 8px;
  }
  .pop::-webkit-scrollbar-thumb {
    background: var(--bg-track);
    border-radius: 4px;
  }
  .pop-item {
    height: 28px;
    box-sizing: border-box;
    display: block;
    line-height: 26px;
    padding: 0 12px;
    margin-top: 2px;
    border: none;
    background: transparent;
    border-radius: 0; /* 原生 select 弹层高亮是直角，不带圆角 */
    color: var(--text);
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    flex: 0 0 auto;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pop-item:first-child {
    margin-top: 0;
  }
  /* 选中/悬停对齐原生 select 弹层：浅色高亮块 + 深色文字 */
  .pop-item:hover,
  .pop-item.selected {
    background: var(--sel-bg);
    color: var(--sel-fg);
  }
</style>