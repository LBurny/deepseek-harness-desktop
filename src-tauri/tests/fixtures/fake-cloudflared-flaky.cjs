// Mock cloudflared（先失败 N 次后成功款）：供隧道监督"慢速重试期一次 Up 即恢复"
// 的集成测试使用。工作目录下 fake-cloudflared.fail-times 指定前多少次 spawn 必死
// （不打印 URL、非零退出），之后按 fake-cloudflared.cjs 的成功款行为打印 URL 并存活。
// 尝试次数经 fake-cloudflared.attempts 文件跨进程累计（每轮 spawn 都是新进程）。
const fs = require('node:fs')
const path = require('node:path')

const attemptsMarker = path.join(process.cwd(), 'fake-cloudflared.attempts')
let attempts = 0
if (fs.existsSync(attemptsMarker)) {
  attempts = Number(fs.readFileSync(attemptsMarker, 'utf8').trim()) || 0
}
attempts += 1
fs.writeFileSync(attemptsMarker, String(attempts))

const failTimesMarker = path.join(process.cwd(), 'fake-cloudflared.fail-times')
const failTimes = fs.existsSync(failTimesMarker)
  ? Number(fs.readFileSync(failTimesMarker, 'utf8').trim()) || 0
  : 0

if (attempts <= failTimes) {
  console.log(`2026-08-17T00:00:00Z INF Starting tunnel tunnelID=fake-flaky attempt=${attempts}`)
  process.exit(1)
}

const urlOverride = path.join(process.cwd(), 'fake-cloudflared.url')
const url = fs.existsSync(urlOverride)
  ? fs.readFileSync(urlOverride, 'utf8').trim()
  : 'https://abc-def-123.trycloudflare.com'

console.log('2026-08-17T00:00:00Z INF Starting tunnel tunnelID=fake-flaky')
console.log('2026-08-17T00:00:00Z INF Version 2026.8.2')
console.log('2026-08-17T00:00:00Z INF +--------------------------------------------------------------------------------------------+')
console.log('2026-08-17T00:00:00Z INF |  Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  |')
console.log(`2026-08-17T00:00:00Z INF |  ${url}`)
console.log('2026-08-17T00:00:00Z INF +--------------------------------------------------------------------------------------------+')

// 保持进程存活
setInterval(() => {}, 10000)
