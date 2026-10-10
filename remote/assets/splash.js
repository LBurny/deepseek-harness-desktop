/*
 * DSHDesktop 远程访问加载过渡页（splash）的卸载脚本：proxy.rs 把它随 splash
 * 标记一起注入到 <div id="root"></div> 之后——注入点即执行点，此时 #root 与
 * splash 都已存在，无需等 DOMContentLoaded。
 *
 * 卸载时机：React 挂载完成 = #root 出现子节点（createRoot 只动 #root 内部，
 * 不碰兄弟节点，splash 不会被 React 抹掉）。检测到后加 .dsh-splash-done
 * 淡出（CSS transition 0.3s），450ms 后移除节点。
 *
 * 语言按 navigator.language 填提示文案（<html lang> 是 Vite 模板恒 en，
 * 不可靠）。任何异常静默：splash 留着不消失与原本白屏等价，不更糟。
 */
;(() => {
  try {
    const splash = document.getElementById('dsh-splash')
    const root = document.getElementById('root')
    if (!splash || !root) return
    const zh = (navigator.language || '').toLowerCase().startsWith('zh')
    const hint = splash.querySelector('.dsh-splash-hint')
    if (hint)
      hint.textContent = zh
        ? '首次连接需下载约 5 MB，网络较慢时可能要等几十秒'
        : 'First connection downloads ~5 MB and may take a while on slow networks'
    let finished = false
    const done = () => {
      if (finished) return
      finished = true
      splash.classList.add('dsh-splash-done')
      setTimeout(() => splash.remove(), 450)
    }
    // 注入脚本执行时 SPA bundle 通常还没下载完，#root 必为空；分支只是兜底
    if (root.childElementCount > 0) return done()
    new MutationObserver(() => {
      if (root.childElementCount > 0) done()
    }).observe(root, { childList: true })
  } catch {
    /* 静默 */
  }
})()
