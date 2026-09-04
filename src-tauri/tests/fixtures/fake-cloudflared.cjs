// Mock cloudflared：供 tunnel 监督的集成测试使用。
// 输出几行假日志后打印 trycloudflare URL（模拟 quick tunnel 就绪），随后保持存活。
// 工作目录下存在 fake-cloudflared.exit-after 文件时，按其毫秒数延迟自杀（模拟崩溃）。
// 工作目录下存在 fake-cloudflared.url 文件时，打印其内容作为隧道 URL
// （模拟重生后换新域名——真实 quick tunnel 每次 spawn 域名都变）。
const fs = require('node:fs')
const path = require('node:path')

const urlOverride = path.join(process.cwd(), 'fake-cloudflared.url')
const url = fs.existsSync(urlOverride)
  ? fs.readFileSync(urlOverride, 'utf8').trim()
  : 'https://abc-def-123.trycloudflare.com'

console.log('2026-08-17T00:00:00Z INF Starting tunnel tunnelID=fake')
console.log('2026-08-17T00:00:00Z INF Version 2026.8.2')
console.log('2026-08-17T00:00:00Z INF +--------------------------------------------------------------------------------------------+')
console.log('2026-08-17T00:00:00Z INF |  Your quick Tunnel has been created! Visit it at (it may take some time to be reachable):  |')
console.log(`2026-08-17T00:00:00Z INF |  ${url}`)
console.log('2026-08-17T00:00:00Z INF +--------------------------------------------------------------------------------------------+')

const marker = path.join(process.cwd(), 'fake-cloudflared.exit-after')
if (fs.existsSync(marker)) {
  const ms = Number(fs.readFileSync(marker, 'utf8').trim())
  if (ms > 0) setTimeout(() => process.exit(1), ms)
}

// 保持进程存活
setInterval(() => {}, 10000)
