// Mock dsh：供 process/notify 的集成测试使用。
// 解析 --port，起 HTTP 服务；GET / 返回 200；/api/events.mux 返回 SSE 心跳。
// 工作目录下存在 fake-dsh.exit-after 文件时，按其毫秒数延迟自杀（模拟崩溃）。
const http = require('node:http')
const fs = require('node:fs')
const path = require('node:path')
const crypto = require('node:crypto')

const portIdx = process.argv.indexOf('--port')
const port = Number(process.argv[portIdx + 1] || 3080)

// 模拟真实 dsh 的浏览器信任栅栏（dsh-client-connection isTrustedApiRequest）：
// /api/* 请求若 sec-fetch-site: cross-site，或 Origin 的 host 与 Host 头不一致 → 403。
// 远程代理若不剥这些浏览器标记头，RPC 调用会全部 403。
function fencePass(req) {
  if (!req.headers.host) return false
  if (req.headers['sec-fetch-site'] === 'cross-site') return false
  const origin = req.headers.origin
  if (origin === undefined) return true
  try {
    return new URL(origin).host === req.headers.host
  } catch {
    return false
  }
}

const server = http.createServer((req, res) => {
  // 插件 client bundle 模拟：含 dsh 内测声明的持久化选择三元式
  //（远程源页面用 memory → 每次访问都弹窗），供代理改写测试
  if (req.url.startsWith('/plugins/fake/client.js')) {
    res.writeHead(200, { 'content-type': 'application/javascript; charset=utf-8' })
    res.end('const w = new WelcomeNoticeStore(connection.api, connection.isLoopback ? "host" : "memory");\n')
    return
  }
  if (req.url === '/api/events.mux' || req.url === '/api/events.host') {
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' })
    res.write(': connected\n\n')
    const timer = setInterval(() => {
      res.write('data: {"type":"heartbeat"}\n\n')
    }, 2000)
    // 每 10s 发一个可通知事件，供通知桥接手动验收
    const notifier = setInterval(() => {
      res.write('data: {"type":"task.complete","text":"fake done"}\n\n')
    }, 10000)
    req.on('close', () => {
      clearInterval(timer)
      clearInterval(notifier)
    })
    return
  }
  if (req.url.startsWith('/api/')) {
    if (!fencePass(req)) {
      res.writeHead(403, { 'content-type': 'text/plain' })
      res.end('forbidden')
      return
    }
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end('{"ok":true}')
    return
  }
  // SPA 入口文档模拟：真实 dsh 对所有非 /api 路径回同一 text/html 文档，
  // 供代理移动端样式注入测试
  if (req.url === '/app') {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    res.end('<!doctype html><html><head><meta charset="utf-8"><title>fake</title></head><body><div id="root"></div></body></html>')
    return
  }
  // 无 </head> 的 HTML：注入应静默跳过、原文透传
  if (req.url === '/app-nohead') {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    res.end('<!doctype html><html><body>no head</body></html>')
    return
  }
  res.writeHead(200, { 'content-type': 'text/plain' })
  res.end('ok')
})

server.listen(port, '127.0.0.1', () => {
  console.log(`listening http://127.0.0.1:${port}`)
  // 0.1.2 BrowserAuth：就绪后 stdout 打印带 launch token 的 URL（壳的 token 唯一来源）
  console.log(`dsh web: http://127.0.0.1:${port}/?token=fixture-token-0123456789abcdef`)
})

// WebSocket 下行流：与真实 dsh 一致，/api/events.mux 与 /api/events.host 通过 WS 推送
// {"type":"server-request","method":<事件类型>,"payload":{...}} 帧。
const WS_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11'
server.on('upgrade', (req, socket) => {
  if (req.url !== '/api/events.mux' && req.url !== '/api/events.host') {
    socket.destroy()
    return
  }
  const accept = crypto
    .createHash('sha1')
    .update(req.headers['sec-websocket-key'] + WS_GUID)
    .digest('base64')
  socket.write(
    'HTTP/1.1 101 Switching Protocols\r\n' +
      'Upgrade: websocket\r\n' +
      'Connection: Upgrade\r\n' +
      `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
  )
  const send = (frame) => {
    const payload = Buffer.from(JSON.stringify({ type: 'server-request', rpcId: 'x', ...frame }))
    const len = payload.length
    const header = len < 126
      ? Buffer.from([0x81, len])
      : Buffer.from([0x81, 126, (len >> 8) & 0xff, len & 0xff])
    socket.write(Buffer.concat([header, payload]))
  }
  const sessionEvent = (sessionId, event) => ({
    method: 'session/event',
    payload: { type: 'session/event', sessionId, event },
  })
  const turnEnd = (seq, kind) => ({
    type: 'turn/end',
    seq,
    time: 0,
    data: { turn: 1, reason: { kind } },
  })
  const turnStart = (seq) => ({ type: 'turn/start', seq, time: 0, data: { turn: 1 } })
  const toolCall = (seq) => ({
    type: 'tool/call',
    seq,
    time: 0,
    data: { callId: `c${seq}`, name: 'bash', arguments: '{}' },
  })
  const title = (seq, text) => ({ type: 'session/title', seq, time: 0, data: { title: text } })

  const timers = []
  if (req.url === '/api/events.mux') {
    timers.push(setInterval(() => send({ method: 'heartbeat', payload: {} }), 2000))
    // 每 3s 发一个待批准事件，供通知桥接验收
    timers.push(setInterval(() => send({ method: 'approval/requested', payload: {} }), 3000))
    // 每 2s 一轮事件（首轮延迟 1s，让 host 流的子代理标记先到位）：
    //   主会话干活回合（turn/start → tool/call → completed）→ 任务完成
    //   主会话纯回答回合（无 tool/call 的 completed）        → 回答完成
    //   主会话中止回合                                       → 静默
    //   子代理完成                                           → 过滤
    const cycle = () => {
      send(sessionEvent('fx-main', title(1, 'fx 主会话')))
      send(sessionEvent('fx-main', turnStart(2)))
      send(sessionEvent('fx-main', toolCall(3)))
      send(sessionEvent('fx-main', turnEnd(4, 'completed')))
      send(sessionEvent('fx-main', turnStart(5)))
      send(sessionEvent('fx-main', turnEnd(6, 'completed')))
      send(sessionEvent('fx-main', turnEnd(7, 'aborted')))
      send(sessionEvent('fx-sub', title(1, 'fx 子代理')))
      send(sessionEvent('fx-sub', turnStart(2)))
      send(sessionEvent('fx-sub', toolCall(3)))
      send(sessionEvent('fx-sub', turnEnd(4, 'completed')))
    }
    timers.push(setTimeout(() => {
      cycle()
      timers.push(setInterval(cycle, 2000))
    }, 1000))
  } else {
    // host：连接即推 + 每 2s 重推子代理标记（fx-sub 是子代理会话）
    const added = () =>
      send({
        method: 'host/session-added',
        payload: { type: 'host/session-added', sessionId: 'fx-sub', blank: false, origin: 'subagent' },
      })
    added()
    timers.push(setInterval(added, 2000))
  }
  socket.on('close', () => timers.forEach((t) => clearInterval(t)))
  socket.on('error', () => {})
})

const marker = path.join(process.cwd(), 'fake-dsh.exit-after')
if (fs.existsSync(marker)) {
  const ms = Number(fs.readFileSync(marker, 'utf8').trim())
  if (ms > 0) setTimeout(() => process.exit(1), ms)
}
