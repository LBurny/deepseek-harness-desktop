/*
 * DSHDesktop 远程访问移动端增强：
 * 1) 会话页"对话/轨迹"旁追加"信息"标签（≤700px），点开展示回合统计面板
 *    （轮步/LLM 耗时/首 token/缓存/token 用量）。
 * 2) 会话页"信息"前追加"项目"标签（远程全宽可用）：iframe 指向代理托管的
 *    项目浏览页 /__dsh-desktop/project，手机端浏览当前会话工作区的文件树
 *    并预览图片/md/代码。桌面壳直连 dsh 不经代理，天然无此标签。
 * 3) 输入卡片工具行（"+" 旁）注入回形针附件按钮，手机端从系统文件
 *    选择器传图片进草稿（上游只有拖拽/剪贴板两条入口，手机都没有）。
 * 4) 视口复位：iOS WKWebView 键盘收起后页面停在无法手势复位的平移残留上
 *    （头部与标签栏停在视口外，观感如全屏），输入框失焦/视口变化时复位文档
 *    滚动并强制重排。
 *
 * 设计要点：
 * - 渐进增强：任何一步找不到目标节点就静默放弃——mobile.css 里统计行的
 *   两行换行样式是兜底，本脚本失效时信息仍在输入区下方。
 * - 统计节点不搬家，用 MutationObserver 克隆同步：React 对被移走的节点
 *   removeChild 会抛 NotFoundError（切会话卸载统计行时必崩）。
 * - 增强生效（标签已挂上）后给 <html> 打 data-dshmobile-enhanced，
 *   mobile.css 借此隐藏输入区下方的原统计行。
 * - 面板打开时只动视觉：原生标签的激活态被 CSS 降级，React 状态不受影响；
 *   点"对话/轨迹"（捕获期监听，两个面板共用一次接线）退出面板。
 */
(() => {
  try {
    const TAB_ATTR = 'data-dshmobile-tab'
    const PANEL_ATTR = 'data-dshmobile-info'
    const OPEN_ATTR = 'data-dshmobile-info-open'
    const ENHANCED_ATTR = 'data-dshmobile-enhanced'
    const PROJECT_TAB_ATTR = 'data-dshmobile-project-tab'
    const PROJECT_PANEL_ATTR = 'data-dshmobile-project'
    const PROJECT_OPEN_ATTR = 'data-dshmobile-project-open'
    const PROJECT_URL = '/__dsh-desktop/project'
    const CLOSE_WIRED = 'data-dshmobile-closewired'

    const narrow = () => matchMedia('(max-width: 700px)').matches
    const zh = () => (document.documentElement.lang || '').toLowerCase().startsWith('zh')
    const L = {
      info: () => (zh() ? '信息' : 'Info'),
      project: () => (zh() ? '项目' : 'Project'),
      title: () => (zh() ? '回合统计' : 'Turn stats'),
      empty: () =>
        zh()
          ? '暂无回合统计——完成一轮对话后，这里会显示耗时与 token 用量。'
          : 'No turn stats yet. Finish a turn and timing/token usage shows up here.',
    }

    // 统计行 = composer 里的 StatsPills 行，上游给它挂了稳定钩子 data-composer-stats
    // （dsh-client-ui-chat，回合数/步数 + token 用量两枚药丸）。0.1.5 的分隔点"·"
    // 在药丸 label **内部**，旧版"root 直接子代含 ≥2 个 _sep"的形状已不存在——
    // 旧锚点会让本函数找不到行（信息页永远显示空态）且 mobile.css 的隐藏规则
    // 一并失配（原行留在输入区下方），双双改到这个钩子上。
    const findStatsRoot = (chatRoot) => {
      const row = chatRoot.querySelector('[data-composer-stats]')
      if (row) return row
      // 兜底：统计行还没渲染（steps=0 且无 token 用量时上游返回 null），交给空态文案
      return null
    }

    // 关闭全部增强面板（信息/项目互斥，且都让位给原生标签）
    const closePanels = (tablist, chatRoot) => {
      chatRoot.removeAttribute(OPEN_ATTR)
      chatRoot.removeAttribute(PROJECT_OPEN_ATTR)
      tablist.removeAttribute('data-dshmobile-open')
      for (const b of tablist.querySelectorAll(`[${TAB_ATTR}][data-active], [${PROJECT_TAB_ATTR}][data-active]`))
        b.removeAttribute('data-active')
    }
    // 捕获期：点原生"对话/轨迹"退出增强面板（React 自己的标签切换不受影响）。
    // 每个 tablist 只挂一次（信息/项目两个 setup 都会调）
    const ensureCloseWiring = (tablist, chatRoot) => {
      if (tablist.hasAttribute(CLOSE_WIRED)) return
      tablist.setAttribute(CLOSE_WIRED, '')
      tablist.addEventListener(
        'click',
        (e) => {
          const t = e.target.closest('[role="tab"]')
          if (!t || t.hasAttribute(TAB_ATTR) || t.hasAttribute(PROJECT_TAB_ATTR)) return
          closePanels(tablist, chatRoot)
        },
        true,
      )
    }
    // 打开某面板：先关另一个，top 对齐头部底，激活态视觉转移（React 状态不动）
    const activate = (tablist, chatRoot, header, btn, panel, openAttr) => {
      closePanels(tablist, chatRoot)
      panel.style.top = `${header.offsetHeight}px`
      chatRoot.setAttribute(openAttr, '')
      tablist.setAttribute('data-dshmobile-open', '')
      btn.setAttribute('data-active', '')
    }
    // 标签按钮：类名克隆自原生标签（摘掉激活态类），视觉与"对话/轨迹"一致
    const mountTab = (tablist, attr, label, before) => {
      const anyTab = tablist.querySelector('[role="tab"]')
      const btn = document.createElement('button')
      btn.type = 'button'
      btn.setAttribute('role', 'tab')
      btn.setAttribute(attr, '')
      btn.setAttribute('aria-selected', 'false')
      btn.className = anyTab.className.replace(/\S*_tabActive\S*/g, '').trim()
      btn.textContent = label
      tablist.insertBefore(btn, before || null)
      return btn
    }

    const setupInfo = (tablist) => {
      const chatRoot = tablist.closest('[class*="_root"]')
      const header = tablist.closest('header')
      if (!chatRoot || !header) return
      ensureCloseWiring(tablist, chatRoot)

      const btn = mountTab(tablist, TAB_ATTR, L.info(), null)

      // 信息面板：绝对定位盖在会话区上（顶边 = 头部高），输入条 z7 在其上仍可输入
      const panel = document.createElement('div')
      panel.setAttribute(PANEL_ATTR, '')
      const card = document.createElement('div')
      card.setAttribute('data-dshmobile-info-card', '')
      const h2 = document.createElement('h2')
      h2.textContent = L.title()
      const body = document.createElement('div')
      body.setAttribute('data-dshmobile-info-body', '')
      card.appendChild(h2)
      card.appendChild(body)
      panel.appendChild(card)
      chatRoot.appendChild(panel)

      const syncStats = () => {
        const stats = findStatsRoot(chatRoot)
        let clone = body.querySelector('[data-dshmobile-stats]')
        if (!stats) {
          if (clone) clone.remove()
          if (!body.querySelector('[data-dshmobile-empty]')) {
            const empty = document.createElement('p')
            empty.setAttribute('data-dshmobile-empty', '')
            empty.textContent = L.empty()
            body.appendChild(empty)
          }
          return
        }
        if (!clone) {
          body.innerHTML = ''
          clone = document.createElement('div')
          clone.setAttribute('data-dshmobile-stats', '')
          body.appendChild(clone)
        }
        if (clone.className !== stats.className) clone.className = stats.className
        if (clone.innerHTML !== stats.innerHTML) clone.innerHTML = stats.innerHTML
      }

      btn.addEventListener('click', () => {
        activate(tablist, chatRoot, header, btn, panel, OPEN_ATTR)
        syncStats()
      })

      // 统计行文本随回合推进更新（秒级 tick 与回合结束），面板开着时跟随同步
      new MutationObserver(() => {
        if (chatRoot.hasAttribute(OPEN_ATTR)) syncStats()
      }).observe(chatRoot, { subtree: true, childList: true, characterData: true })

      // 标签挂上了：增强生效，原统计行改由信息页呈现（CSS 隐藏输入区下方的原行）。
      // 跟随断点：旋屏/拉窗离开 ≤700px 时摘掉增强标记，原统计行恢复显示，
      // 否则宽屏下标签被 CSS 隐藏、原行也被隐藏，统计就无处可见了。
      const mq = matchMedia('(max-width: 700px)')
      const applyEnhanced = () => {
        if (mq.matches) document.documentElement.setAttribute(ENHANCED_ATTR, '')
        else document.documentElement.removeAttribute(ENHANCED_ATTR)
      }
      mq.addEventListener('change', applyEnhanced)
      applyEnhanced()
    }

    // 项目标签：面板是 iframe 指向代理托管的项目页（同源，页面自读当前会话）。
    // 不受 700px 断点限制（远程宽屏也可用；桌面壳不经代理永远看不到）。
    // 首次点开才挂 iframe src（不点不加载），之后保活（树/预览状态保留）。
    const setupProject = (tablist) => {
      const chatRoot = tablist.closest('[class*="_root"]')
      const header = tablist.closest('header')
      if (!chatRoot || !header) return
      ensureCloseWiring(tablist, chatRoot)
      // 插在信息标签之前（信息可能尚未注入→append；顺序恒为 对话/轨迹/项目/信息）
      const btn = mountTab(
        tablist,
        PROJECT_TAB_ATTR,
        L.project(),
        tablist.querySelector(`[${TAB_ATTR}]`),
      )
      const panel = document.createElement('div')
      panel.setAttribute(PROJECT_PANEL_ATTR, '')
      chatRoot.appendChild(panel)
      btn.addEventListener('click', () => {
        if (!panel.firstChild) {
          const f = document.createElement('iframe')
          f.src = PROJECT_URL
          f.setAttribute('title', L.project())
          panel.appendChild(f)
        }
        activate(tablist, chatRoot, header, btn, panel, PROJECT_OPEN_ATTR)
      })
    }

    const ensure = () => {
      const tablist = document.querySelector('header [role="tablist"]')
      if (!tablist) return
      // 项目标签先行：即使信息标签后挂，顺序仍是 对话/轨迹/项目/信息
      if (!tablist.querySelector(`[${PROJECT_TAB_ATTR}]`)) setupProject(tablist)
      // 信息标签维持 ≤700px 门控（宽屏统计行有原生位置，面板反而遮内容）
      if (narrow() && !tablist.querySelector(`[${TAB_ATTR}]`)) setupInfo(tablist)
    }

    // 视图随导航挂载/卸载：监听文档子树，标签栏出现且没挂过就挂
    new MutationObserver(() => {
      try {
        ensure()
      } catch {
        /* 静默 */
      }
    }).observe(document.documentElement, { subtree: true, childList: true })
    ensure()
  } catch {
    /* 静默：增强失败不影响页面 */
  }
})()

/*
 * 附件按钮（**0.1.5 起退化为兜底**）：0.1.2 时代上游输入只有拖拽/剪贴板两条图片
 * 入口（onPaste → intakeImages → createDraftImages → base64 → session.prompt），
 * 手机浏览器一条都没有；这里在输入卡片工具行（"+" 旁）注入一个样式克隆自 "+" 的
 * 回形针按钮，点击调起系统文件选择器，选完构造 DataTransfer 合成 paste 事件喂回
 * 上游自己的粘贴管线——类型/数量/体积校验与报错 toast 全部复用上游逻辑。
 *
 * **0.1.5 起上游自带附件按钮**（实测 aria-label「添加附件」，支持任意文件类型），
 * 我们的图片版（「添加图片附件」）与它并排就成了两个回形针。故 setup() 先探测原生
 * 入口：存在即不注入、并摘掉可能残留的旧按钮；上游哪天再撤掉该入口，这里自动回退
 * 到注入（探测只看 aria-label 里的「附件 / attach」，改名不匹配时最多多注入一个
 * 按钮，功能不损）。
 *
 * 注意：上游 host（dsh-attachment admitEncodedImages，sharp 校验）只认
 * png/jpeg/webp/gif 四种位图，文档类型上游不支持，选择器因此只开图片。
 * 按钮与 file input 都是我们加的异物节点，input 放 body 下（React 树外），
 * 按钮插入 tools 行（与信息标签同款先例：只加不移 React 节点）；
 * React 重渲染抹掉按钮时由观察器按 DOM 缺席重挂。
 */
;(() => {
  try {
    const BTN_ATTR = 'data-dshmobile-attach'
    const narrow = () => matchMedia('(max-width: 700px)').matches
    const zh = () => (document.documentElement.lang || '').toLowerCase().startsWith('zh')
    // Feather 风格回形针（24 视窗描边图标，与 dsh 的 outline 图标同风）
    const PAPERCLIP_SVG =
      '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/></svg>'

    // 共享一个 file input（body 下、React 树外）；点击时记录目标卡片。
    // 惰性创建：本脚本注入点在 </head> 前，执行时 body 可能还没出来。
    let picker = null
    let targetCard = null
    const ensurePicker = () => {
      if (picker) return picker
      if (!document.body) return null
      picker = document.createElement('input')
      picker.type = 'file'
      picker.accept = 'image/png,image/jpeg,image/webp,image/gif'
      picker.multiple = true
      picker.style.display = 'none'
      picker.addEventListener('change', () => {
        const files = Array.from(picker.files || [])
        picker.value = ''
        const card = targetCard
        targetCard = null
        if (files.length === 0 || !card || !card.isConnected) return
        // 0.1.2 起输入框从 textarea 换成 contenteditable——两形态都要认
        const editor = card.querySelector(
          'textarea, [contenteditable="true"], [role="textbox"]',
        )
        if (!editor) return
        try {
          const dt = new DataTransfer()
          for (const f of files) dt.items.add(f)
          editor.dispatchEvent(
            new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }),
          )
        } catch {
          /* 静默 */
        }
      })
      document.body.appendChild(picker)
      return picker
    }

    // 上游是否已自带附件入口（0.1.5 起为「添加附件」，支持任意文件类型）。
    // 只看 aria-label 里的「附件 / attach」且排除我们自己的按钮——上游改名则
    // 退化为"多注入一个按钮"，不损功能。
    const hasNativeAttach = (tools) =>
      Array.from(tools.querySelectorAll('button')).some(
        (b) =>
          !b.hasAttribute(BTN_ATTR) && /附件|attach/i.test(b.getAttribute('aria-label') || ''),
      )

    const setup = (tools) => {
      const addBtn = tools.querySelector('button[class*="_add"]')
      const card = tools.closest('[class*="_card"]')
      if (!addBtn || !card) return
      // 输入体存在性：textarea（≤0.1.1）或 contenteditable（0.1.2 起）
      if (!card.querySelector('textarea, [contenteditable="true"], [role="textbox"]'))
        return
      // "+" 可能包在 Tooltip 的 wrapper 里：锚定它在 tools 行的直接子代
      let anchor = addBtn
      while (anchor.parentElement && anchor.parentElement !== tools) anchor = anchor.parentElement

      const btn = document.createElement('button')
      btn.type = 'button'
      btn.className = addBtn.className // 克隆 "+" 的类名，28px 圆形同款
      btn.disabled = addBtn.disabled
      btn.setAttribute(BTN_ATTR, '')
      btn.setAttribute('aria-label', zh() ? '添加图片附件' : 'Add image attachment')
      btn.innerHTML = PAPERCLIP_SVG
      anchor.after(btn)

      // 与上游 keepFocus 一致：按下不抢输入框焦点
      btn.addEventListener('mousedown', (e) => e.preventDefault())
      btn.addEventListener('click', () => {
        if (btn.disabled) return
        const p = ensurePicker()
        if (!p) return
        targetCard = card
        p.value = ''
        p.click()
      })
    }

    const ensure = () => {
      // 离开窄断点（旋屏/拉窗）时摘除按钮，桌面形态保持原生
      if (!narrow()) {
        for (const b of document.querySelectorAll(`button[${BTN_ATTR}]`)) b.remove()
        return
      }
      for (const tools of document.querySelectorAll('div[class*="_tools"]')) {
        try {
          // 0.1.5 起上游自带附件按钮（任意文件类型）：原生入口在就不注入我们的
          // 图片版（否则两个回形针并排），并摘掉可能残留的旧按钮
          if (hasNativeAttach(tools)) {
            for (const b of tools.querySelectorAll(`button[${BTN_ATTR}]`)) b.remove()
            continue
          }
          const existing = tools.querySelector(`button[${BTN_ATTR}]`)
          if (existing) {
            // 跟随 "+" 的禁用态（锁定/忙时上游 onPaste 也会拒收，双保险）
            const addBtn = tools.querySelector('button[class*="_add"]')
            if (addBtn) existing.disabled = addBtn.disabled
            continue
          }
          setup(tools)
        } catch {
          /* 静默 */
        }
      }
    }

    new MutationObserver(() => {
      try {
        ensure()
      } catch {
        /* 静默 */
      }
    }).observe(document.documentElement, { subtree: true, childList: true })
    ensure()
  } catch {
    /* 静默：增强失败不影响页面 */
  }
})()


/*
 * 视口复位：iOS WKWebView（微信内置浏览器实测）在底部输入框聚焦→键盘收起后，
 * 页面会停在一段无法用手势复位的平移残留上——头部（会话标题与 对话/轨迹/项目/
 * 信息 标签栏）停在视口之外，观感如"进入了全屏模式"，且上下滑动失灵。
 * dsh 自身没有键盘视口处理（bundle 里仅 react-dom 引用 visualViewport，桌面
 * Chromium/模拟键盘均无法复现），因此在增强层自愈：输入框失焦与键盘视口变化
 * 时，把文档滚动复位到 0 并强制一次重排，逼 WebKit 撤销平移残留。健康状态下
 * 文档滚动恒为 0，复位是无操作；聚焦期间的平移（键盘弹出时露出光标）是合法
 * 行为，不干预。
 */
;(() => {
  try {
    const EDITOR = 'textarea, [contenteditable="true"], [role="textbox"]'
    let keyboard = false
    const stuck = () =>
      window.scrollY !== 0 ||
      window.scrollX !== 0 ||
      document.documentElement.scrollTop !== 0 ||
      document.body.scrollTop !== 0
    const reset = () => {
      if (keyboard || !stuck()) return
      window.scrollTo(0, 0)
      document.documentElement.scrollTop = 0
      document.body.scrollTop = 0
      void document.body.offsetHeight // 强制重排：逼 WebKit 重新夹紧视口平移
    }
    document.addEventListener(
      'focusin',
      (e) => {
        if (e.target.closest?.(EDITOR)) keyboard = true
      },
      true,
    )
    document.addEventListener(
      'focusout',
      (e) => {
        if (!e.target.closest?.(EDITOR)) return
        keyboard = false
        // 键盘收起带动画：零头几次定时补位，覆盖动画结束后的最终布局
        for (const ms of [0, 350, 900]) setTimeout(reset, ms)
      },
      true,
    )
    // 键盘弹出/收起都走 visualViewport resize：收起且无聚焦时复位
    window.visualViewport?.addEventListener('resize', () => {
      if (!keyboard) setTimeout(reset, 100)
    })
  } catch {
    /* 静默：增强失败不影响页面 */
  }
})()
